//! Persistent workspace, home and state directories, under a named storage when one is asked for.
//!
//! Each directory has a protected node-owned record; neither a unit's removal nor a restart removes its files.

use crate::units::{HostConfig, io_error};
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::volumes::{StorageInfo, VolumeRequest};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// One volume's exact directory, kept outside directories writable by the environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Volume {
    /// The owned name, storage and labels.
    pub request: VolumeRequest,
    /// Its actual persistent directory.
    pub path: PathBuf,
}

/// Refuses traversal and anything that cannot be one owned directory name.
pub fn check_name(name: &str) -> Result<(), DriverError> {
    if name.is_empty()
        || name.starts_with('.')
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(DriverError::InvalidSpec(
            "a host unit or volume needs one plain name".into(),
        ));
    }
    Ok(())
}

pub(crate) fn metadata(config: &HostConfig, name: &str) -> Result<PathBuf, DriverError> {
    check_name(name)?;
    Ok(config
        .base
        .join("volume-records")
        .join(format!("{name}.json")))
}

pub(crate) fn read(config: &HostConfig, name: &str) -> Result<Option<Volume>, DriverError> {
    let path = metadata(config, name)?;
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|_| {
            DriverError::Internal("a host volume record is damaged and was left intact".into())
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error)),
    }
}

/// An absolute directory is created without accepting a symlink at its owned root.
pub fn directory(path: &Path) -> Result<(), DriverError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(DriverError::InvalidSpec(
            "a host directory is an absolute normalized path".into(),
        ));
    }
    std::fs::create_dir_all(path).map_err(io_error)?;
    let actual = std::fs::symlink_metadata(path).map_err(io_error)?;
    if !actual.is_dir() || actual.file_type().is_symlink() {
        return Err(DriverError::InvalidSpec(
            "an owned host directory must be a real directory".into(),
        ));
    }
    Ok(())
}

/// Ensures only a directory whose stored owner and storage match the request.
pub fn ensure(config: &HostConfig, request: &VolumeRequest) -> Result<Volume, DriverError> {
    if let Some(volume) = read(config, &request.name)? {
        if volume.request.storage != request.storage
            || request.labels.iter().any(|(key, value)| {
                if matches!(key.as_str(), "orch.runtime_id" | "orch.carried") {
                    return false;
                }
                if matches!(
                    key.as_str(),
                    "orch.system"
                        | "orch.environment_id"
                        | "orch.provider_id"
                        | "orch.mount_id"
                        | "orch.recovery_job_id"
                ) {
                    return volume.request.labels.get(key) != Some(value);
                }
                volume
                    .request
                    .labels
                    .get(key)
                    .is_some_and(|held| held != value)
            })
        {
            return Err(DriverError::InvalidSpec(
                "the named host volume belongs to a different owner or storage".into(),
            ));
        }
        if !volume.path.is_dir() {
            return Err(DriverError::NotFound(
                "an owned host volume directory is missing; it is not silently replaced".into(),
            ));
        }
        return Ok(volume);
    }
    let base = match request.storage.as_deref() {
        Some(name)
            if config.storages.is_empty()
                && name == orch_driver_contract::volumes::DEFAULT_STORAGE =>
        {
            config.base.join("volumes")
        }
        Some(name) => config
            .storages
            .iter()
            .find(|storage| storage.name == name)
            .map(|storage| storage.path.clone())
            .ok_or_else(|| {
                DriverError::InvalidSpec("the named host storage is not configured".into())
            })?,
        None => config
            .storages
            .iter()
            .find(|storage| storage.default)
            .map_or_else(
                || config.base.join("volumes"),
                |storage| storage.path.clone(),
            ),
    };
    directory(&base)?;
    let path = base.join(&request.name);
    if path.exists() {
        return Err(DriverError::InvalidSpec(
            "an unrecorded host directory with this volume name already exists".into(),
        ));
    }
    directory(&path)?;
    let volume = Volume {
        request: request.clone(),
        path,
    };
    crate::units::write_json(&metadata(config, &request.name)?, &volume)?;
    Ok(volume)
}

/// A kept directory is checked without creating any missing bytes or accepting another owner's record.
pub fn exists(config: &HostConfig, request: &VolumeRequest) -> Result<bool, DriverError> {
    Ok(read(config, &request.name)?.is_some_and(|volume| {
        volume.path.is_dir()
            && volume.request.storage == request.storage
            && request
                .labels
                .iter()
                .all(|(key, value)| volume.request.labels.get(key) == Some(value))
    }))
}

/// Removes only the recorded owned directory; a symlink is refused and a missing directory is harmless.
pub fn remove(config: &HostConfig, name: &str) -> Result<(), DriverError> {
    let Some(volume) = read(config, name)? else {
        return Ok(());
    };
    if let Ok(actual) = std::fs::symlink_metadata(&volume.path) {
        if actual.file_type().is_symlink() || !actual.is_dir() {
            return Err(DriverError::InvalidSpec(
                "the kept volume's directory changed identity".into(),
            ));
        }
        std::fs::remove_dir_all(&volume.path).map_err(io_error)?;
    }
    std::fs::remove_file(metadata(config, name)?).map_err(io_error)
}

/// Named storages report native available space, with no claim that host processes have private overlays.
#[must_use]
pub fn storages(config: &HostConfig) -> Vec<StorageInfo> {
    let named = if config.storages.is_empty() {
        vec![crate::units::HostStorage {
            name: orch_driver_contract::volumes::DEFAULT_STORAGE.into(),
            path: config.base.join("volumes"),
            default: true,
        }]
    } else {
        config.storages.clone()
    };
    named
        .iter()
        .map(|storage| {
            #[cfg(unix)]
            let sizes = rustix::fs::statvfs(&storage.path)
                .ok()
                .map(|stat| {
                    const BYTES_PER_GB: u64 = 1_000_000_000;
                    let gb = |blocks: u64| {
                        i64::try_from(blocks.saturating_mul(stat.f_frsize.max(1)) / BYTES_PER_GB)
                            .unwrap_or(i64::MAX)
                    };
                    (gb(stat.f_blocks), gb(stat.f_bavail))
                })
                .unwrap_or((0, 0));
            #[cfg(not(unix))]
            let sizes = (0, 0);
            StorageInfo {
                name: storage.name.clone(),
                default: storage.default,
                capacity_gb: sizes.0,
                free_gb: sizes.1,
                private_views: false,
            }
        })
        .collect()
}
