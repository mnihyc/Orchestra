//! Images by reference with digest pinning, and images loaded from an archive.
//!
//! A unit's file system comes from one of three sources. A **reference** is pulled from its registry, with the
//! credentials the operator configured, and a template that pins a digest is made from exactly that image; one that
//! does not follows its tag and is pulled again whenever the image is made available (a host without a registry
//! keeps the image it has). An **archive** is loaded once for each blob and each image in it, and tagged with a name
//! of the driver's own so that it is found again; a **build** is in [`crate::build`]. A unit is made from the image
//! the spec names, found by the same rule that made it available, so the image a unit runs is the one that was
//! asked for and nothing newer is ever put behind it.

use bollard::auth::DockerCredentials;
use bollard::models::{
    ContainerCreateBody, HostConfig, HostConfigLogConfig, ImageInspect, ImageSummary,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, ImportImageOptionsBuilder,
    ListImagesOptionsBuilder, RemoveContainerOptionsBuilder, RemoveImageOptionsBuilder,
    StartContainerOptionsBuilder, TagImageOptionsBuilder, WaitContainerOptionsBuilder,
};
use futures::StreamExt;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::runtime::RuntimeSpec;
use orch_driver_contract::sources::{ArchiveSource, ImageEnsured, Source, SourceKind};
use orch_types::{Hash, Json};
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;
use tokio_util::io::ReaderStream;

use crate::client::{daemon_capacity_failure, map_error, status_of};
use crate::config::DockerConfig;
use crate::driver::DockerDriver;
use crate::progress::{LogTail, Phase, ProgressSink};
use crate::spec::DockerSpec;

/// How long the daemon may take to answer a request that starts a pull or a load: only the beginning of the
/// answer is timed, but a load takes the whole upload to begin, and an archive can be large.
pub const TRANSFER_REQUEST_TIMEOUT: Duration = Duration::from_secs(3_600);

/// The tag a pull takes when the reference names no tag and no digest.
const DEFAULT_TAG: &str = "latest";

/// The names the daemon gives Docker Hub.
const HUB_HOSTS: [&str; 2] = ["docker.io", "index.docker.io"];

/// A shell only checks executable paths; a slow or unusable image leaves the facts unknown.
const PROGRAM_PROBE_WAIT: Duration = Duration::from_secs(30);
/// Enough for the shell and its runtime, without letting an image inspection consume a host.
const PROGRAM_PROBE_MEMORY: i64 = 128 * 1024 * 1024;
/// The check needs one shell, with room for interpreters used by alternative images.
const PROGRAM_PROBE_PIDS: i64 = 16;
/// A path lookup needs little CPU; one processor keeps a defective image bounded.
const PROGRAM_PROBE_CPUS: i64 = 1_000_000_000;
/// The exit code encodes absent Docker and QEMU as two bits, so no image output is retained.
const PROGRAM_PROBE_SCRIPT: &str = r#"docker=1
qemu=2
command -v docker >/dev/null 2>&1 && command -v dockerd >/dev/null 2>&1 && docker=0
command -v qemu-system-x86_64 >/dev/null 2>&1 && command -v qemu-system-aarch64 >/dev/null 2>&1 && qemu=0
exit $((docker + qemu))"#;
/// Bits assigned by the inspection script, distinct from a shell's failed launch status.
const DOCKER_ABSENT: i64 = 1;
const QEMU_ABSENT: i64 = 2;

fn programs_from_status(status: Option<i64>) -> BTreeMap<String, bool> {
    match status {
        Some(status) if (0..=(DOCKER_ABSENT | QEMU_ABSENT)).contains(&status) => [
            ("docker".to_owned(), status & DOCKER_ABSENT == 0),
            ("qemu".to_owned(), status & QEMU_ABSENT == 0),
        ]
        .into(),
        _ => BTreeMap::new(),
    }
}

/// A reference taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// The repository, with its registry when it has one: `ghcr.io/org/image`, `busybox`.
    pub name: String,
    /// The tag, when there is one.
    pub tag: Option<String>,
    /// The digest, when there is one.
    pub digest: Option<String>,
}

/// Takes a reference apart: `name[:tag][@digest]`. A colon is a tag only after the last slash, since before it
/// it is a registry's port.
#[must_use]
pub fn parse_reference(reference: &str) -> Reference {
    let (rest, digest) = match reference.split_once('@') {
        Some((rest, digest)) => (rest, Some(digest.to_owned())),
        None => (reference, None),
    };
    let after_slash = rest.rfind('/').map_or(0, |slash| slash + 1);
    match rest[after_slash..].rfind(':') {
        Some(colon) => {
            let at = after_slash + colon;
            Reference {
                name: rest[..at].to_owned(),
                tag: Some(rest[at + 1..].to_owned()),
                digest,
            }
        }
        None => Reference {
            name: rest.to_owned(),
            tag: None,
            digest,
        },
    }
}

/// The reference a unit is made from: the repository at the digest when one is pinned, otherwise the reference
/// as it was written.
#[must_use]
pub fn pinned_reference(reference: &str, digest: Option<&str>) -> String {
    match digest {
        Some(digest) => format!("{}@{digest}", parse_reference(reference).name),
        None => reference.to_owned(),
    }
}

/// The registry a repository is on: the first part of its name when that looks like a host, otherwise the Hub.
#[must_use]
pub fn registry_of(name: &str) -> &str {
    match name.split_once('/') {
        Some((first, _)) if first.contains('.') || first.contains(':') || first == "localhost" => {
            first
        }
        _ => HUB_HOSTS[0],
    }
}

/// A repository name with the parts the Hub leaves out: `docker.io/library/busybox` and `busybox` are one.
fn plain_name(name: &str) -> &str {
    let name = HUB_HOSTS
        .iter()
        .find_map(|host| name.strip_prefix(&format!("{host}/")).map(str::to_owned))
        .map_or(name, |_| name);
    let name = name
        .strip_prefix("docker.io/")
        .or_else(|| name.strip_prefix("index.docker.io/"))
        .unwrap_or(name);
    name.strip_prefix("library/").unwrap_or(name)
}

/// The digest an image is known by: the one a registry gave for the repository `reference` names, else any it
/// has, else its identifier (which, where the daemon keeps images with containerd, is the manifest's digest).
#[must_use]
pub fn digest_of(image: &ImageInspect, reference: &str) -> Option<String> {
    let wanted = parse_reference(reference).name;
    let digests = image.repo_digests.as_deref().unwrap_or_default();
    let of_repository = digests.iter().find_map(|entry| {
        let (name, digest) = entry.split_once('@')?;
        (plain_name(name) == plain_name(&wanted)).then(|| digest.to_owned())
    });
    of_repository
        .or_else(|| {
            digests
                .first()
                .and_then(|entry| entry.split_once('@'))
                .map(|(_, digest)| digest.to_owned())
        })
        .or_else(|| image.id.clone().filter(|id| id.starts_with("sha256:")))
}

/// The name an archive's image is kept under once it is loaded.
#[must_use]
pub fn archive_alias(config: &DockerConfig, archive: &ArchiveSource) -> String {
    let which = Hash::of(archive.image.as_deref().unwrap_or_default().as_bytes()).to_string();
    format!(
        "{}:archive-{}-{}",
        config.image_repository,
        archive.blob,
        &which[..16]
    )
}

/// The credentials configured for the registry of a repository.
fn credentials_for(config: &DockerConfig, name: &str) -> Option<DockerCredentials> {
    let registry = registry_of(name);
    config
        .registries
        .iter()
        .find(|auth| {
            auth.server == registry
                || (HUB_HOSTS.contains(&registry) && HUB_HOSTS.contains(&auth.server.as_str()))
        })
        .map(|auth| DockerCredentials {
            username: auth.username.clone(),
            password: auth.password.clone(),
            identitytoken: auth.identity_token.clone(),
            serveraddress: Some(auth.server.clone()),
            ..DockerCredentials::default()
        })
}

/// A failed pull or load distinguishes host capacity from an image or registry failure.
fn image_error(error: bollard::errors::Error) -> DriverError {
    match error {
        bollard::errors::Error::DockerResponseServerError { message, .. }
        | bollard::errors::Error::DockerStreamError { error: message } => {
            if daemon_capacity_failure(&message) {
                DriverError::NoCapacity(message)
            } else {
                DriverError::Image(message)
            }
        }
        other => map_error(other),
    }
}

impl DockerDriver {
    /// Observes programs on the exact image without its entrypoint, hooks, network, mounts or capabilities.
    pub(crate) async fn image_programs(&self, image: &ImageInspect) -> BTreeMap<String, bool> {
        let Some(image) = image.id.clone() else {
            return BTreeMap::new();
        };
        let daemon = self.daemon.clone();
        let labels = crate::units::driver_labels(&self.config)
            .into_iter()
            .collect();
        // Finish the bounded probe and its cleanup even if the calling image job is cancelled.
        tokio::spawn(async move {
            let name = format!("orch-image-probe-{}", orch_types::new_id());
            let body = ContainerCreateBody {
                image: Some(image),
                user: Some("0".to_owned()),
                working_dir: Some("/".to_owned()),
                entrypoint: Some(vec!["/bin/sh".to_owned()]),
                cmd: Some(vec!["-c".to_owned(), PROGRAM_PROBE_SCRIPT.to_owned()]),
                labels: Some(labels),
                host_config: Some(HostConfig {
                    network_mode: Some("none".to_owned()),
                    readonly_rootfs: Some(true),
                    cap_drop: Some(vec!["ALL".to_owned()]),
                    security_opt: Some(vec!["no-new-privileges".to_owned()]),
                    memory: Some(PROGRAM_PROBE_MEMORY),
                    memory_swap: Some(PROGRAM_PROBE_MEMORY),
                    nano_cpus: Some(PROGRAM_PROBE_CPUS),
                    pids_limit: Some(PROGRAM_PROBE_PIDS),
                    log_config: Some(HostConfigLogConfig {
                        typ: Some("none".to_owned()),
                        ..HostConfigLogConfig::default()
                    }),
                    ..HostConfig::default()
                }),
                ..ContainerCreateBody::default()
            };
            let probe = async {
                daemon
                    .docker()
                    .create_container(
                        Some(CreateContainerOptionsBuilder::default().name(&name).build()),
                        body,
                    )
                    .await
                    .ok()?;
                daemon
                    .docker()
                    .start_container(&name, Some(StartContainerOptionsBuilder::default().build()))
                    .await
                    .ok()?;
                let docker = daemon.docker();
                let mut waiting =
                    Box::pin(docker.wait_container(
                        &name,
                        Some(WaitContainerOptionsBuilder::default().build()),
                    ));
                match waiting.next().await {
                    Some(Ok(done)) => Some(done.status_code),
                    Some(Err(bollard::errors::Error::DockerContainerWaitError {
                        code, ..
                    })) => Some(code),
                    _ => None,
                }
            };
            let status = tokio::time::timeout(PROGRAM_PROBE_WAIT, probe)
                .await
                .ok()
                .flatten();
            let cleanup = daemon.docker().remove_container(
                &name,
                Some(
                    RemoveContainerOptionsBuilder::default()
                        .force(true)
                        .v(true)
                        .build(),
                ),
            );
            if !matches!(
                tokio::time::timeout(PROGRAM_PROBE_WAIT, cleanup).await,
                Ok(Ok(()))
            ) {
                tracing::warn!(%name, "the image program probe could not be removed");
            }
            programs_from_status(status)
        })
        .await
        .unwrap_or_default()
    }

    /// The reference a unit of this request is made from, after seeing that the host has the image; see
    /// [`DockerDriver::ensure_image_reporting`] for how it gets there.
    pub(crate) async fn image_for(
        &self,
        spec: &RuntimeSpec,
        docker: &DockerSpec,
    ) -> Result<String, DriverError> {
        if let Some(kept) = self.kept_source(spec).await? {
            return Ok(kept);
        }
        match &docker.source {
            Source::Image { reference, digest } => {
                if reference.starts_with("orch-keep-") {
                    return Err(DriverError::InvalidSpec(
                        "an environment's kept image is selected only by its recreate or restore"
                            .into(),
                    ));
                }
                let original = pinned_reference(reference, digest.as_deref());
                let target = self.config.local_image(&original).to_owned();
                if self.daemon.image(&target).await?.is_none() {
                    if self.config.offline_images.contains_key(&original) {
                        return Err(DriverError::Image(format!("carried-in image {reference} is missing; load its verified archive on this daemon")));
                    }
                    self.pull_image(reference, digest.as_deref(), &crate::progress::Silent)
                        .await?;
                }
                Ok(target)
            }
            Source::Build(build) => {
                let alias = crate::build::spec_alias(&self.config, build);
                self.require_made(&alias, "built").await?;
                Ok(alias)
            }
            Source::Archive(archive) => {
                let alias = archive_alias(&self.config, archive);
                self.require_made(&alias, "loaded").await?;
                Ok(alias)
            }
        }
        .inspect(|image| tracing::debug!(unit = %spec.name, %image, "the image of a unit"))
    }

    async fn require_made(&self, alias: &str, how: &str) -> Result<(), DriverError> {
        if self.daemon.image(alias).await?.is_none() {
            return Err(DriverError::Image(format!(
                "the image has not been {how} on this host; make it available first"
            )));
        }
        Ok(())
    }

    /// Pulls an image, and reports what the registry says as it goes.
    pub(crate) async fn pull_image(
        &self,
        reference: &str,
        digest: Option<&str>,
        progress: &dyn ProgressSink,
    ) -> Result<(), DriverError> {
        let parsed = parse_reference(reference);
        let tag = digest
            .map(str::to_owned)
            .or_else(|| parsed.digest.clone())
            .or_else(|| parsed.tag.clone())
            .unwrap_or_else(|| DEFAULT_TAG.to_owned());
        let options = CreateImageOptionsBuilder::default()
            .from_image(&parsed.name)
            .tag(&tag)
            .build();
        let credentials = credentials_for(&self.config, &parsed.name);
        let patient = self.daemon.patient(TRANSFER_REQUEST_TIMEOUT);
        let mut stream = Box::pin(patient.create_image(Some(options), None, credentials));
        let mut last: HashMap<String, String> = HashMap::new();
        let mut log = LogTail::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok(info) => {
                    let id = info.id.clone().unwrap_or_default();
                    if let Some(status) = info.status {
                        let status = self.config.redact_registry_text(&status);
                        // The counters of a download change a hundred times a second; only a change of what a
                        // layer is doing is a line of progress.
                        if last.get(&id) != Some(&status) {
                            let line = if id.is_empty() {
                                status.clone()
                            } else {
                                format!("{id}: {status}")
                            };
                            progress.line(Phase::Pull, &line);
                            log.push(&format!("{line}\n"));
                            last.insert(id, status);
                        }
                    }
                }
                Err(error) => {
                    let failure = image_error(error);
                    return Err(match failure {
                        DriverError::Image(message) => DriverError::Image(
                            self.config
                                .redact_registry_text(&format!("{reference}: {message}")),
                        ),
                        DriverError::NoCapacity(message) => DriverError::NoCapacity(
                            self.config
                                .redact_registry_text(&format!("{reference}: {message}")),
                        ),
                        other => other,
                    });
                }
            }
        }
        Ok(())
    }

    /// Makes the source of a spec available on this host; see `EnvironmentDriver::ensure_image` and
    /// [`DockerDriver::ensure_image_reporting`].
    pub(crate) async fn ensure_source(
        &self,
        spec: &Json,
        progress: &dyn ProgressSink,
    ) -> Result<ImageEnsured, DriverError> {
        let source =
            Source::from_spec(spec).map_err(|error| DriverError::InvalidSpec(error.to_string()))?;
        if source.kind() == SourceKind::Build && !self.config.builds {
            return Err(DriverError::Unsupported(
                "this host does not build images".to_owned(),
            ));
        }
        match source {
            Source::Image { reference, digest } => {
                let original = pinned_reference(&reference, digest.as_deref());
                let target = self.config.local_image(&original).to_owned();
                let carried = self.config.offline_images.contains_key(&original);
                let held = self.daemon.image(&target).await?;
                if carried && held.is_none() {
                    return Err(DriverError::Image(format!(
                        "carried-in image {reference} is missing; load its verified archive on this daemon"
                    )));
                }
                // A pinned image the host holds is the image; a tag is looked up again, and the image the host
                // holds stands in for a registry that cannot be reached.
                let image = match (
                    held,
                    digest.is_some() || carried || reference.contains("@sha256:"),
                ) {
                    (Some(image), true) => image,
                    (held, _) => match self
                        .pull_image(&reference, digest.as_deref(), progress)
                        .await
                    {
                        Ok(()) => self.daemon.image(&target).await?.ok_or_else(|| {
                            DriverError::Image(format!(
                                "{target} is not on this host after it was pulled"
                            ))
                        })?,
                        Err(DriverError::Image(message)) if held.is_some() => {
                            tracing::warn!(%reference, %message, "the registry could not be reached; the image the host holds is used");
                            held.ok_or(DriverError::Image(message))?
                        }
                        Err(error) => return Err(error),
                    },
                };
                Ok(ImageEnsured {
                    programs: self.image_programs(&image).await,
                    digest: if carried {
                        image.id.clone()
                    } else {
                        digest_of(&image, &reference)
                    },
                    built: None,
                })
            }
            Source::Build(build) => self.ensure_build(&build, progress).await,
            Source::Archive(archive) => self.ensure_archive(&archive, progress).await,
        }
    }

    /// Loads an image archive once for each blob and image.
    async fn ensure_archive(
        &self,
        archive: &ArchiveSource,
        progress: &dyn ProgressSink,
    ) -> Result<ImageEnsured, DriverError> {
        let alias = archive_alias(&self.config, archive);
        if let Some(found) = self.daemon.image(&alias).await? {
            return Ok(ImageEnsured {
                programs: self.image_programs(&found).await,
                digest: digest_of(&found, &alias),
                built: None,
            });
        }
        let blobs = self.blobs.as_ref().ok_or_else(|| {
            DriverError::Image("this host has no way to fetch the archive".to_owned())
        })?;
        let file = crate::archive::spool(blobs.as_ref(), &archive.blob).await?;
        let content = tokio::fs::File::open(file.path()).await.map_err(|error| {
            DriverError::Internal(format!("the archive could not be read: {error}"))
        })?;
        let body = bollard::body_try_stream(ReaderStream::new(content));
        let patient = self.daemon.patient(TRANSFER_REQUEST_TIMEOUT);
        let mut stream = Box::pin(patient.import_image(
            ImportImageOptionsBuilder::default().quiet(false).build(),
            body,
            None,
        ));
        let mut loaded_names: Vec<String> = Vec::new();
        let mut loaded_ids: Vec<String> = Vec::new();
        let mut log = LogTail::new();
        while let Some(item) = stream.next().await {
            let info = item.map_err(image_error)?;
            if let Some(text) = info.stream.or(info.status) {
                for line in text.lines().filter(|line| !line.is_empty()) {
                    progress.line(Phase::Load, line);
                    log.push(&format!("{line}\n"));
                    if let Some(name) = line.strip_prefix("Loaded image: ") {
                        loaded_names.push(name.trim().to_owned());
                    } else if let Some(id) = line.strip_prefix("Loaded image ID: ") {
                        loaded_ids.push(id.trim().to_owned());
                    }
                }
            }
        }
        let loaded = match (&archive.image, loaded_names.len() + loaded_ids.len()) {
            (_, 0) => {
                return Err(DriverError::Image(format!(
                    "the archive loaded no image:\n{}",
                    log.as_str()
                )));
            }
            (Some(wanted), _) => loaded_names
                .iter()
                .find(|name| plain_name(name) == plain_name(wanted))
                .cloned()
                .ok_or_else(|| {
                    DriverError::InvalidSpec(format!("the archive holds no image named {wanted}"))
                })?,
            (None, 1) => loaded_names
                .first()
                .or_else(|| loaded_ids.first())
                .cloned()
                .unwrap_or_default(),
            (None, many) => {
                return Err(DriverError::InvalidSpec(format!(
                    "the archive holds {many} images; the spec names which"
                )));
            }
        };
        let (repository, tag) = alias
            .split_once(':')
            .unwrap_or((alias.as_str(), DEFAULT_TAG));
        self.daemon
            .docker()
            .tag_image(
                &loaded,
                Some(
                    TagImageOptionsBuilder::default()
                        .repo(repository)
                        .tag(tag)
                        .build(),
                ),
            )
            .await
            .map_err(map_error)?;
        let image = self
            .daemon
            .image(&alias)
            .await?
            .ok_or_else(|| DriverError::Image("the loaded image vanished".to_owned()))?;
        tracing::info!(%alias, "loaded an image archive");
        Ok(ImageEnsured {
            programs: self.image_programs(&image).await,
            digest: digest_of(&image, &alias),
            built: None,
        })
    }

    /// Removes only this exact name or image, keeping every unselected parent layer.
    pub(crate) async fn remove_image_tag(&self, tag: &str) -> Result<Removal, DriverError> {
        let answer = match self.remove_image_alone(tag).await {
            Ok(answer) => answer,
            Err(error) if status_of(&error) == Some(404) => return Ok(Removal::Absent),
            Err(error) => return Err(map_error(error)),
        };
        if !answer.iter().any(|item| item.deleted.is_some()) {
            return Ok(Removal::Renamed);
        }
        Ok(Removal::Gone)
    }

    /// One removal request that leaves the parents alone.
    pub(crate) async fn remove_image_alone(
        &self,
        reference: &str,
    ) -> Result<Vec<bollard::models::ImageDeleteResponseItem>, bollard::errors::Error> {
        self.daemon
            .docker()
            .remove_image(
                reference,
                Some(RemoveImageOptionsBuilder::default().noprune(true).build()),
                None,
            )
            .await
    }

    /// Every image on the host, those a build made on the way too.
    pub(crate) async fn list_all_images(&self) -> Result<Vec<ImageSummary>, DriverError> {
        self.daemon
            .docker()
            .list_images(Some(ListImagesOptionsBuilder::default().all(true).build()))
            .await
            .map_err(map_error)
    }
}

/// What taking a name from an image came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    /// The host had no such image.
    Absent,
    /// The name was taken and the image stays under another one.
    Renamed,
    /// The selected image is gone; unselected parent layers remain.
    Gone,
}

/// What the daemon lists for the tag of an image that has none.
const NO_TAG: &str = "<none>:<none>";

/// What it lists for the digest of an image that has none.
const NO_DIGEST: &str = "<none>@<none>";

/// Whether an image is a step of a build: made on the way, and given no name by anyone.
#[must_use]
pub fn is_step(image: &ImageSummary) -> bool {
    image.repo_tags.iter().all(|tag| tag == NO_TAG)
        && image.repo_digests.iter().all(|digest| digest == NO_DIGEST)
}

/// The build steps that nothing stands on once an image is gone, nearest first so that one goes before the one
/// it stands on. `parent` is the image beneath the one that went and `images` are the ones that are left; the
/// walk goes down while the image is a step and no image that is left, other than the steps already counted,
/// has it as its parent, and ends at the image the build started from (which has a name) or at a step that
/// another build stands on.
#[must_use]
pub fn steps_without_use(images: &[ImageSummary], parent: &str) -> Vec<String> {
    let by_id: HashMap<&str, &ImageSummary> = images
        .iter()
        .map(|image| (image.id.as_str(), image))
        .collect();
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for image in images.iter().filter(|image| !image.parent_id.is_empty()) {
        children
            .entry(image.parent_id.as_str())
            .or_default()
            .push(image.id.as_str());
    }
    let mut steps: Vec<String> = Vec::new();
    let mut current = parent;
    while let Some(image) = by_id.get(current) {
        let stood_on = children.get(current).is_some_and(|above| {
            above
                .iter()
                .any(|child| !steps.iter().any(|step| step == child))
        });
        if !is_step(image) || stood_on {
            break;
        }
        steps.push(image.id.clone());
        current = image.parent_id.as_str();
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_is_taken_apart_into_name_tag_and_digest_and_a_port_is_not_a_tag() {
        let busybox = parse_reference("busybox");
        assert_eq!(
            (busybox.name.as_str(), busybox.tag, busybox.digest),
            ("busybox", None, None)
        );
        let tagged = parse_reference("ghcr.io/o/base:0.1.0");
        assert_eq!(
            (tagged.name.as_str(), tagged.tag.as_deref()),
            ("ghcr.io/o/base", Some("0.1.0"))
        );
        let ported = parse_reference("localhost:5000/team/app");
        assert_eq!(
            (ported.name.as_str(), ported.tag),
            ("localhost:5000/team/app", None)
        );
        let ported_tagged = parse_reference("localhost:5000/team/app:v2");
        assert_eq!(ported_tagged.tag.as_deref(), Some("v2"));
        let pinned = parse_reference("alpine:3@sha256:abc");
        assert_eq!(
            (
                pinned.name.as_str(),
                pinned.tag.as_deref(),
                pinned.digest.as_deref()
            ),
            ("alpine", Some("3"), Some("sha256:abc"))
        );
    }

    #[test]
    fn a_pinned_image_is_the_repository_at_the_digest_and_an_unpinned_one_is_as_written() {
        assert_eq!(
            pinned_reference("ghcr.io/o/base:0.1.0", Some("sha256:abc")),
            "ghcr.io/o/base@sha256:abc"
        );
        assert_eq!(
            pinned_reference("ghcr.io/o/base:0.1.0", None),
            "ghcr.io/o/base:0.1.0"
        );
        assert_eq!(
            pinned_reference("busybox", Some("sha256:1")),
            "busybox@sha256:1"
        );
    }

    #[test]
    fn the_registry_of_a_repository_is_its_first_part_when_that_is_a_host_and_otherwise_the_hub() {
        assert_eq!(registry_of("ghcr.io/o/base"), "ghcr.io");
        assert_eq!(registry_of("localhost:5000/x"), "localhost:5000");
        assert_eq!(registry_of("localhost/x"), "localhost");
        assert_eq!(registry_of("library/busybox"), "docker.io");
        assert_eq!(registry_of("busybox"), "docker.io");
        assert_eq!(
            registry_of("reg.example.com:5000/a"),
            "reg.example.com:5000"
        );
    }

    #[test]
    fn the_digest_of_an_image_is_the_one_its_repository_has_else_the_first_else_its_identifier() {
        let image: ImageInspect = serde_json::from_value(serde_json::json!({
            "Id": "sha256:ee", "RepoDigests": ["ghcr.io/o/other@sha256:11", "busybox@sha256:22"]
        }))
        .unwrap();
        assert_eq!(
            digest_of(&image, "busybox:latest").as_deref(),
            Some("sha256:22")
        );
        assert_eq!(
            digest_of(&image, "docker.io/library/busybox").as_deref(),
            Some("sha256:22")
        );
        assert_eq!(
            digest_of(&image, "unknown/repo").as_deref(),
            Some("sha256:11")
        );
        let local: ImageInspect =
            serde_json::from_value(serde_json::json!({ "Id": "sha256:ee" })).unwrap();
        assert_eq!(digest_of(&local, "x").as_deref(), Some("sha256:ee"));
        assert_eq!(digest_of(&ImageInspect::default(), "x"), None);
    }

    #[test]
    fn an_archive_is_kept_under_a_name_of_the_drivers_own_for_each_blob_and_each_image_in_it() {
        let config = DockerConfig::default();
        let blob = Hash::of(b"archive");
        let one = archive_alias(&config, &ArchiveSource { blob, image: None });
        let other = archive_alias(
            &config,
            &ArchiveSource {
                blob,
                image: Some("app:1".into()),
            },
        );
        assert!(one.starts_with(&format!("orch-build:archive-{blob}-")));
        assert_ne!(one, other);
        assert_eq!(
            one,
            archive_alias(&config, &ArchiveSource { blob, image: None })
        );
    }

    #[test]
    fn credentials_are_those_of_the_registry_the_repository_is_on() {
        let mut config = DockerConfig::default();
        config.registries.push(crate::config::RegistryAuth {
            server: "ghcr.io".into(),
            username: Some("bot".into()),
            password: Some("secret".into()),
            identity_token: None,
        });
        let found = credentials_for(&config, "ghcr.io/o/base").unwrap();
        assert_eq!(found.username.as_deref(), Some("bot"));
        assert_eq!(found.serveraddress.as_deref(), Some("ghcr.io"));
        assert!(credentials_for(&config, "busybox").is_none());
    }

    fn image(id: &str, parent: &str, tags: &[&str]) -> ImageSummary {
        ImageSummary {
            id: id.to_owned(),
            parent_id: parent.to_owned(),
            repo_tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
            ..ImageSummary::default()
        }
    }

    #[test]
    fn the_steps_beneath_a_removed_image_go_with_it_up_to_the_image_the_build_started_from() {
        // base <- one <- two <- three: three was removed, base has a name.
        let left = [
            image("base", "", &["busybox:latest"]),
            image("one", "base", &[]),
            image("two", "one", &["<none>:<none>"]),
        ];
        assert_eq!(steps_without_use(&left, "two"), ["two", "one"]);
    }

    #[test]
    fn a_step_that_another_build_stands_on_stays_with_everything_beneath_it() {
        // Two builds share their first step; the second one is the one that was removed.
        let left = [
            image("base", "", &["busybox:latest"]),
            image("shared", "base", &[]),
            image("first-run", "shared", &[]),
            image("first-final", "first-run", &["app:1"]),
            image("second-run", "shared", &[]),
        ];
        assert_eq!(steps_without_use(&left, "second-run"), ["second-run"]);
        // And when the first one goes as well, the shared step goes with it.
        let alone = [
            image("base", "", &["busybox:latest"]),
            image("shared", "base", &[]),
            image("first-run", "shared", &[]),
        ];
        assert_eq!(
            steps_without_use(&alone, "first-run"),
            ["first-run", "shared"]
        );
    }

    #[test]
    fn nothing_is_removed_beneath_an_image_that_has_a_name_or_that_the_host_does_not_have() {
        let left = [
            image("base", "", &["busybox:latest"]),
            image("pinned", "base", &[]),
        ];
        assert!(steps_without_use(&left, "base").is_empty());
        assert!(steps_without_use(&left, "missing").is_empty());
        let mut digested = image("digested", "base", &[]);
        digested.repo_digests = vec!["busybox@sha256:abc".to_owned()];
        assert!(steps_without_use(&[digested], "digested").is_empty());
    }
}
