//! Owned same-image commits, flattening, archive transfer and removal by exact environment/generation names.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use bollard::models::{ContainerConfig, ImageInspect};
use bollard::query_parameters::{
    CommitContainerOptionsBuilder, ImportImageOptionsBuilder, InspectContainerOptionsBuilder,
    ListImagesOptionsBuilder, RemoveImageOptionsBuilder, TagImageOptionsBuilder,
};
use futures::StreamExt;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::kept::{KeepEstimate, KeptImage};
use orch_driver_contract::runtime::{LABEL_SYSTEM, RuntimeSpec};
use orch_driver_contract::units::UnitRef;
use orch_types::Id;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

use crate::client::{map_error, status_of};
use crate::driver::DockerDriver;
use crate::units::{DRIVER_NAME, LABEL_DRIVER};

const ENVIRONMENT: &str = "orch.kept.environment";
const GENERATION: &str = "orch.kept.generation";
const CAPTURE: &str = "orch.kept.capture";
const LAYERS: &str = "orch.kept.layers";
const BASE_BYTES: &str = "orch.kept.base_bytes";
const KEPT_BYTES: &str = "orch.kept.bytes";
const TOTAL_BYTES: &str = "orch.kept.total_bytes";
// The measured local commit rate is about 30 seconds/GiB plus one second of overhead.
const GIB: i64 = 1 << 30;
const SECONDS_PER_GIB: i64 = 30;
pub const FLATTEN_AFTER: u32 = 10;
// Keep transfers use the existing image transfer budget, including large installed toolchains.
const TRANSFER_WAIT: Duration = crate::images::TRANSFER_REQUEST_TIMEOUT;

pub fn name(environment: Id, generation: i64) -> String {
    format!("orch-keep-{environment}:{generation}")
}

fn labels(image: &ImageInspect) -> Option<&HashMap<String, String>> {
    image.config.as_ref()?.labels.as_ref()
}

fn owned(image: &ImageInspect, reference: &str) -> Option<KeptImage> {
    let labels = labels(image)?;
    if labels.get(LABEL_DRIVER).map(String::as_str) != Some(DRIVER_NAME) {
        return None;
    }
    let system = labels.get(LABEL_SYSTEM)?.clone();
    if system.is_empty() {
        return None;
    }
    let environment = labels.get(ENVIRONMENT)?.parse().ok()?;
    let generation: i64 = labels.get(GENERATION)?.parse().ok()?;
    let capture = labels.get(CAPTURE)?.parse().ok()?;
    if generation < 1 || reference != name(environment, generation) {
        return None;
    }
    let layers = labels.get(LAYERS)?.parse().ok()?;
    let base: i64 = labels.get(BASE_BYTES)?.parse().ok()?;
    let bytes = labels
        .get(KEPT_BYTES)
        .map(|value| value.parse::<i64>())
        .transpose()
        .ok()?
        .unwrap_or_else(|| image.size.unwrap_or(0).saturating_sub(base).max(0));
    let total_bytes = labels
        .get(TOTAL_BYTES)
        .map(|value| value.parse::<i64>())
        .transpose()
        .ok()?
        .unwrap_or_else(|| image.size.unwrap_or(0).max(0));
    if bytes < 0 || total_bytes < bytes {
        return None;
    }
    Some(KeptImage {
        system,
        environment,
        generation,
        capture,
        reference: reference.into(),
        bytes,
        total_bytes,
        layers,
    })
}

fn estimate(bytes: i64, flatten: bool) -> KeepEstimate {
    KeepEstimate {
        bytes: bytes.max(0),
        seconds: 1 + bytes
            .max(0)
            .saturating_mul(SECONDS_PER_GIB)
            .saturating_add(GIB - 1)
            / GIB,
        flatten,
    }
}

impl DockerDriver {
    pub(crate) async fn verify_kept_image(
        &self,
        wanted: &KeptImage,
    ) -> Result<Option<KeptImage>, DriverError> {
        if wanted.reference != name(wanted.environment, wanted.generation) || wanted.generation < 1
        {
            return Err(DriverError::InvalidSpec(
                "the kept image has no canonical environment/generation name".into(),
            ));
        }
        let Some(image) = self.daemon.image(&wanted.reference).await? else {
            return Ok(None);
        };
        let found = owned(&image, &wanted.reference)
            .filter(|found| {
                found.system == wanted.system
                    && found.environment == wanted.environment
                    && found.generation == wanted.generation
            })
            .ok_or_else(|| {
                DriverError::InvalidSpec(
                    "the image name is occupied by an image this environment does not own".into(),
                )
            })?;
        Ok(Some(found))
    }

    pub(crate) async fn kept_source(
        &self,
        spec: &RuntimeSpec,
    ) -> Result<Option<String>, DriverError> {
        let Some(kept) = &spec.kept_image else {
            return Ok(None);
        };
        if spec.labels.get(LABEL_SYSTEM) != Some(&kept.system)
            || spec
                .subject
                .as_ref()
                .and_then(|subject| subject.environment.as_ref())
                .is_some_and(|environment| environment.id != kept.environment)
        {
            return Err(DriverError::InvalidSpec(
                "the kept image belongs to another environment or system".into(),
            ));
        }
        let found = self.verify_kept_image(kept).await?.ok_or_else(|| {
            DriverError::NotFound("the environment's kept image is unavailable on this host".into())
        })?;
        if found.capture != kept.capture {
            return Err(DriverError::InvalidSpec(
                "this kept name now holds another snapshot; the recorded one cannot be substituted"
                    .into(),
            ));
        }
        Ok(Some(found.reference))
    }

    pub async fn estimate_kept_files(&self, unit: &UnitRef) -> Result<KeepEstimate, DriverError> {
        let container = self
            .container_of(unit)
            .await?
            .ok_or_else(|| DriverError::NotFound("the owned unit is absent".into()))?;
        let id = container.id.as_deref().unwrap_or(&unit.reference);
        let sized = self
            .daemon
            .docker()
            .inspect_container(
                id,
                Some(InspectContainerOptionsBuilder::default().size(true).build()),
            )
            .await
            .map_err(map_error)?;
        let parent = self
            .daemon
            .image(container.image.as_deref().unwrap_or_default())
            .await?;
        let count = parent
            .as_ref()
            .and_then(labels)
            .and_then(|labels| labels.get(LAYERS))
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        let flatten = count.saturating_add(1) >= FLATTEN_AFTER;
        let bytes = (if flatten {
            sized.size_root_fs
        } else {
            sized.size_rw
        })
        .filter(|bytes| *bytes >= 0)
        .ok_or_else(|| {
            DriverError::Unsupported(
                "the daemon did not report the filesystem size needed for this capture".into(),
            )
        })?;
        Ok(estimate(bytes, flatten))
    }

    pub async fn record_kept_files(
        &self,
        unit: &UnitRef,
        environment: Id,
        generation: i64,
        capture: Id,
    ) -> Result<KeptImage, DriverError> {
        let _held = self.locks.lock(&unit.name).await;
        let container = self
            .container_of(unit)
            .await?
            .ok_or_else(|| DriverError::NotFound("the owned unit is absent".into()))?;
        if container
            .state
            .as_ref()
            .is_some_and(|state| state.running == Some(true))
        {
            return Err(DriverError::Busy(
                "the unit must stop before its installed files are recorded".into(),
            ));
        }
        if generation < 1 {
            return Err(DriverError::InvalidSpec(
                "a kept generation is positive".into(),
            ));
        }
        let system = container
            .config
            .as_ref()
            .and_then(|config| config.labels.as_ref())
            .and_then(|labels| labels.get(LABEL_SYSTEM))
            .cloned()
            .ok_or_else(|| DriverError::InvalidSpec("the owned unit has no system label".into()))?;
        let reference = name(environment, generation);
        let wanted = KeptImage {
            system: system.clone(),
            environment,
            generation,
            capture,
            reference: reference.clone(),
            bytes: 0,
            total_bytes: 0,
            layers: 0,
        };
        if let Some(found) = self.verify_kept_image(&wanted).await?
            && found.capture == capture
        {
            return Ok(found);
        }
        // A prior failed attempt can reuse a generation, as can archiving the current
        // one. Commit replaces only this verified owned tag. Keep the old image metadata
        // until then: it may be the stopped unit's parent and rollback still needs it.
        let parent = self
            .daemon
            .image(container.image.as_deref().unwrap_or_default())
            .await?
            .ok_or_else(|| DriverError::Image("the unit's parent image is unavailable".into()))?;
        let prior = labels(&parent).filter(|labels| {
            labels.get(ENVIRONMENT) == Some(&environment.to_string())
                && labels.get(LABEL_SYSTEM) == Some(&system)
                && labels.get(LABEL_DRIVER).map(String::as_str) == Some(DRIVER_NAME)
        });
        let layers = prior
            .and_then(|labels| labels.get(LAYERS))
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0)
            .saturating_add(1);
        let flatten = layers >= FLATTEN_AFTER;
        // Measure this filesystem, rather than subtracting two optional image-inspect sizes.
        // Keep the measurement with the snapshot so save/load and another image store agree too.
        let sized = self
            .daemon
            .docker()
            .inspect_container(
                container.id.as_deref().unwrap_or(&unit.reference),
                Some(InspectContainerOptionsBuilder::default().size(true).build()),
            )
            .await
            .map_err(map_error)?;
        let writable = sized.size_rw.filter(|bytes| *bytes >= 0).ok_or_else(|| {
            DriverError::Unsupported(
                "the daemon did not report the stopped unit's writable size".into(),
            )
        })?;
        let root = sized
            .size_root_fs
            .filter(|bytes| *bytes >= 0)
            .ok_or_else(|| {
                DriverError::Unsupported(
                    "the daemon did not report the stopped unit's filesystem size".into(),
                )
            })?;
        let previous_bytes = prior
            .and_then(|labels| labels.get(KEPT_BYTES))
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or_else(|| {
                prior
                    .and_then(|labels| labels.get(BASE_BYTES))
                    .and_then(|value| value.parse::<i64>().ok())
                    .map_or(0, |base| {
                        parent.size.unwrap_or(0).saturating_sub(base).max(0)
                    })
            });
        let previous_total = prior
            .and_then(|labels| labels.get(TOTAL_BYTES))
            .and_then(|value| value.parse::<i64>().ok())
            .or(parent.size.filter(|bytes| *bytes > 0))
            .unwrap_or_else(|| root.saturating_sub(writable).max(0));
        let bytes = if flatten {
            root
        } else {
            previous_bytes.saturating_add(writable)
        };
        let total = if flatten {
            root
        } else {
            previous_total.saturating_add(writable).max(bytes)
        };
        let base = total.saturating_sub(bytes);
        let mut config: ContainerConfig = match parent.config {
            Some(config) => serde_json::from_value(
                serde_json::to_value(config)
                    .map_err(|error| DriverError::Internal(error.to_string()))?,
            )
            .map_err(|error| DriverError::Internal(error.to_string()))?,
            None => ContainerConfig::default(),
        };
        let labels = config.labels.get_or_insert_with(HashMap::new);
        if let Some(unit_labels) = container
            .config
            .as_ref()
            .and_then(|config| config.labels.as_ref())
        {
            labels.extend(unit_labels.clone());
        }
        labels.insert(LABEL_DRIVER.into(), DRIVER_NAME.into());
        labels.insert(LABEL_SYSTEM.into(), system);
        labels.insert(ENVIRONMENT.into(), environment.to_string());
        labels.insert(GENERATION.into(), generation.to_string());
        labels.insert(CAPTURE.into(), capture.to_string());
        labels.insert(LAYERS.into(), if flatten { 1 } else { layers }.to_string());
        labels.insert(BASE_BYTES.into(), base.to_string());
        labels.insert(KEPT_BYTES.into(), bytes.to_string());
        labels.insert(TOTAL_BYTES.into(), total.to_string());
        let result = if flatten {
            self.flatten_kept(
                unit,
                &reference,
                &config,
                parent.os.as_deref().unwrap_or("linux"),
                parent
                    .architecture
                    .as_deref()
                    .unwrap_or(&self.facts().architecture),
            )
            .await
        } else {
            let repository = format!("orch-keep-{environment}");
            self.daemon
                .patient(TRANSFER_WAIT)
                .commit_container(
                    CommitContainerOptionsBuilder::default()
                        .container(&unit.reference)
                        .repo(&repository)
                        .tag(&generation.to_string())
                        .pause(false)
                        .build(),
                    config,
                )
                .await
                .map(|_| ())
                .map_err(map_error)
        };
        if let Err(error) = result {
            // Remove only a name whose complete ownership labels were actually written. The old
            // stopped container and its parent image are untouched and can be started for rollback.
            if self
                .verify_kept_image(&wanted)
                .await?
                .is_some_and(|found| found.capture == capture)
            {
                let _ = self.drop_kept_image(&wanted).await;
            }
            return Err(error);
        }
        self.verify_kept_image(&wanted)
            .await?
            .filter(|found| found.capture == capture)
            .ok_or_else(|| {
                DriverError::Image("the recorded image did not retain this exact snapshot".into())
            })
    }

    async fn flatten_kept(
        &self,
        unit: &UnitRef,
        reference: &str,
        config: &ContainerConfig,
        os: &str,
        architecture: &str,
    ) -> Result<(), DriverError> {
        let archive = tempfile::NamedTempFile::new()
            .map_err(|error| DriverError::Internal(error.to_string()))?;
        let patient = self.daemon.patient(TRANSFER_WAIT);
        let mut stream = patient.export_container(&unit.reference);
        let mut file = tokio::fs::File::create(archive.path())
            .await
            .map_err(|error| DriverError::Internal(error.to_string()))?;
        while let Some(piece) = stream.next().await {
            file.write_all(&piece.map_err(map_error)?)
                .await
                .map_err(|error| DriverError::Internal(error.to_string()))?;
        }
        file.sync_all()
            .await
            .map_err(|error| DriverError::Internal(error.to_string()))?;
        drop(file);
        let saved = tempfile::NamedTempFile::new()
            .map_err(|error| DriverError::Internal(error.to_string()))?;
        let root = archive.path().to_owned();
        let into = saved.path().to_owned();
        let reference = reference.to_owned();
        let config = config.clone();
        let os = os.to_owned();
        let architecture = architecture.to_owned();
        // A one-layer Docker archive preserves exact JSON configuration, including literal
        // dollar signs/newlines in Env. No Dockerfile parser may expand or reinterpret it.
        tokio::task::spawn_blocking(move || {
            flat_archive(&root, &into, &reference, &config, &os, &architecture)
        })
        .await
        .map_err(|error| DriverError::Internal(error.to_string()))?
        .map_err(|error| DriverError::Internal(error.to_string()))?;
        let file = tokio::fs::File::open(saved.path())
            .await
            .map_err(|error| DriverError::Internal(error.to_string()))?;
        let mut import = Box::pin(patient.import_image(
            ImportImageOptionsBuilder::default().quiet(false).build(),
            bollard::body_try_stream(ReaderStream::new(file)),
            None,
        ));
        while let Some(item) = import.next().await {
            item.map_err(map_error)?;
        }
        Ok(())
    }

    pub async fn list_kept_images(&self, system: &str) -> Result<Vec<KeptImage>, DriverError> {
        let filters = HashMap::from([(
            "label".to_owned(),
            vec![
                format!("{LABEL_DRIVER}={DRIVER_NAME}"),
                format!("{LABEL_SYSTEM}={system}"),
                ENVIRONMENT.into(),
            ],
        )]);
        let images = self
            .daemon
            .docker()
            .list_images(Some(
                ListImagesOptionsBuilder::default()
                    .filters(&filters)
                    .all(true)
                    .build(),
            ))
            .await
            .map_err(map_error)?;
        let mut kept = Vec::new();
        for image in images {
            let image = self.daemon.image(&image.id).await?;
            let Some(image) = image else {
                continue;
            };
            for reference in image.repo_tags.iter().flatten() {
                if let Some(found) = owned(&image, reference).filter(|found| found.system == system)
                {
                    kept.push(found);
                }
            }
        }
        kept.sort_by_key(|image| (image.environment, image.generation));
        Ok(kept)
    }

    pub async fn drop_kept_image(&self, image: &KeptImage) -> Result<(), DriverError> {
        let Some(found) = self.verify_kept_image(image).await? else {
            return Ok(());
        };
        // A delayed failure cleanup never removes a later attempt that reused this generation's name.
        if found.capture != image.capture {
            return Ok(());
        }
        match self
            .daemon
            .docker()
            .remove_image(
                &image.reference,
                Some(
                    RemoveImageOptionsBuilder::default()
                        .force(true)
                        .noprune(true)
                        .build(),
                ),
                None,
            )
            .await
        {
            Ok(_) => {
                tracing::info!(environment=%image.environment,generation=image.generation,image=%image.reference,"removed an unused kept image name");
                Ok(())
            }
            Err(error) if status_of(&error) == Some(404) => Ok(()),
            Err(error) => Err(map_error(error)),
        }
    }

    /// No general daemon prune: only complete Orch labels, no other tags, and Docker itself
    /// refuses deleting metadata a container or a later image still references.
    pub async fn sweep_kept_metadata(
        &self,
        system: &str,
        owners: &[orch_driver_contract::kept::KeptOwner],
    ) -> Result<usize, DriverError> {
        let filters = HashMap::from([(
            "label".to_owned(),
            vec![
                format!("{LABEL_DRIVER}={DRIVER_NAME}"),
                format!("{LABEL_SYSTEM}={system}"),
                ENVIRONMENT.into(),
            ],
        )]);
        let images = self
            .daemon
            .docker()
            .list_images(Some(
                ListImagesOptionsBuilder::default()
                    .filters(&filters)
                    .all(true)
                    .build(),
            ))
            .await
            .map_err(map_error)?;
        let mut removed = 0;
        for summary in images {
            let Some(image) = self.daemon.image(&summary.id).await? else {
                continue;
            };
            if image
                .repo_tags
                .iter()
                .flatten()
                .any(|tag| tag != "<none>:<none>")
            {
                continue;
            }
            let Some(environment) = labels(&image)
                .and_then(|labels| labels.get(ENVIRONMENT))
                .and_then(|id| id.parse().ok())
            else {
                continue;
            };
            let Some(generation) = labels(&image)
                .and_then(|labels| labels.get(GENERATION))
                .and_then(|generation| generation.parse().ok())
            else {
                continue;
            };
            let Some(owned) = owned(&image, &name(environment, generation))
                .filter(|owned| owned.system == system)
            else {
                continue;
            };
            if owners.iter().any(|owner| {
                owner.environment == owned.environment
                    && owner.generation == owned.generation
                    && owner.capture.is_none_or(|capture| capture == owned.capture)
            }) {
                if self.daemon.image(&owned.reference).await?.is_none() {
                    self.daemon
                        .docker()
                        .tag_image(
                            &summary.id,
                            Some(
                                TagImageOptionsBuilder::default()
                                    .repo(&format!("orch-keep-{environment}"))
                                    .tag(&generation.to_string())
                                    .build(),
                            ),
                        )
                        .await
                        .map_err(map_error)?;
                    removed += 1;
                    tracing::info!(%environment,generation,image=%summary.id,"restored the retained kept-image name");
                }
                continue;
            }
            match self
                .daemon
                .docker()
                .remove_image(
                    &summary.id,
                    Some(
                        RemoveImageOptionsBuilder::default()
                            .force(false)
                            .noprune(true)
                            .build(),
                    ),
                    None,
                )
                .await
            {
                Ok(_) => {
                    removed += 1;
                    tracing::info!(%environment,generation,image=%summary.id,"removed unused owned kept-image metadata");
                }
                Err(error) if matches!(status_of(&error), Some(404 | 409)) => {}
                Err(error) => return Err(map_error(error)),
            }
        }
        Ok(removed)
    }

    pub async fn save_kept_image(&self, image: &KeptImage, into: &Path) -> Result<(), DriverError> {
        self.verify_kept_image(image)
            .await?
            .filter(|found| found.capture == image.capture)
            .ok_or_else(|| {
                DriverError::NotFound("the exact kept snapshot is unavailable".into())
            })?;
        let patient = self.daemon.patient(TRANSFER_WAIT);
        let mut stream = patient.export_image(&image.reference);
        let mut file = tokio::fs::File::create(into)
            .await
            .map_err(|error| DriverError::Internal(error.to_string()))?;
        while let Some(piece) = stream.next().await {
            file.write_all(&piece.map_err(map_error)?)
                .await
                .map_err(|error| DriverError::Internal(error.to_string()))?;
        }
        file.sync_all()
            .await
            .map_err(|error| DriverError::Internal(error.to_string()))
    }

    pub async fn load_kept_image(&self, image: &KeptImage, from: &Path) -> Result<(), DriverError> {
        if let Some(found) = self.verify_kept_image(image).await? {
            return if found.capture == image.capture {
                Ok(())
            } else {
                Err(DriverError::InvalidSpec(
                    "the destination kept name belongs to another snapshot".into(),
                ))
            };
        }
        let file = tokio::fs::File::open(from)
            .await
            .map_err(|error| DriverError::Internal(error.to_string()))?;
        let patient = self.daemon.patient(TRANSFER_WAIT);
        let mut stream = Box::pin(patient.import_image(
            ImportImageOptionsBuilder::default().quiet(false).build(),
            bollard::body_try_stream(ReaderStream::new(file)),
            None,
        ));
        while let Some(item) = stream.next().await {
            item.map_err(map_error)?;
        }
        self.verify_kept_image(image)
            .await?
            .filter(|found| found.capture == image.capture)
            .ok_or_else(|| {
                DriverError::Image(
                    "the transferred archive did not contain this environment's exact kept image"
                        .into(),
                )
            })?;
        Ok(())
    }
}

fn flat_archive(
    root: &Path,
    into: &Path,
    reference: &str,
    config: &ContainerConfig,
    os: &str,
    architecture: &str,
) -> std::io::Result<()> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(root)?;
    let mut hash = Sha256::new();
    // Fixed 64-KiB chunks keep hashing memory independent of the installed filesystem's size.
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let config = serde_json::to_vec(
        &serde_json::json!({"os":os,"architecture":architecture,"config":config,"rootfs":{"type":"layers","diff_ids":[format!("sha256:{}",hex::encode(hash.finalize()))]},"history":[{"created_by":"Orch kept filesystem"}]}),
    )?;
    let config_name = format!("{}.json", hex::encode(Sha256::digest(&config)));
    let manifest = serde_json::to_vec(
        &serde_json::json!([{"Config":config_name,"RepoTags":[reference],"Layers":["layer.tar"]}]),
    )?;
    let mut output = tar::Builder::new(std::fs::File::create(into)?);
    for (name, bytes) in [
        (&config_name[..], config.as_slice()),
        ("manifest.json", manifest.as_slice()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_cksum();
        output.append_data(&mut header, name, bytes)?;
    }
    output.append_file("layer.tar", &mut std::fs::File::open(root)?)?;
    output.into_inner()?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_person_gets_the_measured_keep_estimate_and_an_environment_owns_one_canonical_name() {
        let id = orch_types::new_id();
        assert_eq!(name(id, 3), format!("orch-keep-{id}:3"));
        assert_eq!(estimate(0, false).seconds, 1);
        assert_eq!(estimate(GIB, false).seconds, 31);
        assert!(estimate(2 * GIB, true).flatten);
        assert_eq!(FLATTEN_AFTER, 10);
    }
}
