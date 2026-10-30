//! Mounts in place: paths as they are, volumes as directories under the mounts base.
//!
//! In-place roots retain their existing location; volume mounts use their recorded named-storage path.
//! This driver creates no namespace or private overlay and never reconstructs a path from a template.

use orch_driver_contract::driver::DriverError;
use orch_driver_contract::mounts::{AttachedMount, MountKind};
use orch_driver_contract::runtime::RuntimeSpec;
use std::path::PathBuf;

/// Resolves the attached roots without moving a host path or claiming a private view.
pub fn attached(
    spec: &RuntimeSpec,
    config: &crate::units::HostConfig,
) -> Result<Vec<AttachedMount>, DriverError> {
    spec.mounts
        .iter()
        .map(|mount| {
            let path = match mount.kind {
                MountKind::Path => PathBuf::from(&mount.source),
                MountKind::Volume => {
                    crate::volumes::read(config, &mount.source)?
                        .ok_or_else(|| {
                            DriverError::NotFound(
                                "the mount's owned volume has not been made on this machine".into(),
                            )
                        })?
                        .path
                }
            };
            if !path.is_absolute() {
                return Err(DriverError::InvalidSpec(
                    "an in-place mount is an absolute local path".into(),
                ));
            }
            if !path.exists() {
                if mount.kind == MountKind::Path {
                    return Err(DriverError::NotFound(
                        "an in-place host path does not exist".into(),
                    ));
                }
                std::fs::create_dir_all(&path).map_err(crate::units::io_error)?;
            }
            Ok(AttachedMount {
                name: mount.name.clone(),
                path: path.to_string_lossy().into_owned(),
                mode: orch_driver_contract::mounts::attach_mode(mount.mode, false),
            })
        })
        .collect()
}
