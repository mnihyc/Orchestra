//! Whole mount data operations use held native directories and command-owned workers.

use crate::error::{HostError, Result};
use crate::snapshot::Progress;
use orch_driver_contract::{runtime::DirectoryBinding, volumes::VolumeMeasure};
use std::sync::Arc;

#[cfg(target_os = "linux")]
mod export;
#[cfg(target_os = "linux")]
mod import;

/// Restores a checked whole-volume stream into its original, separately authorized owned root.
pub async fn import(
    binding: DirectoryBinding,
    archive: orch_link_contract::commands::VolumeArchive,
    source: &dyn crate::blobs::BlobSource,
    progress: Arc<dyn Progress>,
) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        import::run(binding, archive, source, progress).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (binding, archive, source, progress);
        Err(HostError::Unsupported(
            "whole mount data needs the machine's native directory identity support".into(),
        ))
    }
}

/// Streams the whole mount into independently checked blob parts, retaining native metadata.
pub async fn export(
    binding: DirectoryBinding,
    part_bytes: i64,
    sink: &dyn crate::blobs::BlobSink,
    progress: Arc<dyn Progress>,
) -> Result<orch_link_contract::commands::VolumeArchive> {
    #[cfg(target_os = "linux")]
    {
        export::run(binding, part_bytes, sink, progress).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (binding, part_bytes, sink, progress);
        Err(HostError::Unsupported(
            "whole mount data needs the machine's native directory identity support".into(),
        ))
    }
}

/// Counts every native entry without workspace ignores, uploads or a tree-sized listing.
pub async fn measure(
    binding: DirectoryBinding,
    progress: Arc<dyn Progress>,
) -> Result<VolumeMeasure> {
    #[cfg(target_os = "linux")]
    {
        crate::error::blocking(move || linux::measure(&binding, progress.as_ref())).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (binding, progress);
        Err(HostError::Unsupported(
            "whole mount data needs the machine's native directory identity support".into(),
        ))
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::{
        paths::{PathStyle, Workspace},
        platform::posix::Dir,
    };
    use std::{path::Path, time::Instant};

    pub(super) fn measure(
        binding: &DirectoryBinding,
        progress: &dyn Progress,
    ) -> Result<VolumeMeasure> {
        let root = Workspace::open(Path::new(&binding.path), &[], PathStyle::Posix, true)?;
        let actual = root.root_meta()?;
        if binding.device.zip(binding.inode) != Some((actual.dev, actual.ino)) {
            return Err(HostError::Unsupported(
                "the original mount directory changed identity".into(),
            ));
        }
        let directory = root.root_dir()?;
        let mut stack = vec![(
            directory
                .names_cursor()
                .map_err(|e| HostError::io("reading a mount directory", e))?,
            directory,
        )];
        let (mut files, mut bytes, mut directories) = (0_i64, 0_i64, 1_i64);
        let mut reported = Instant::now();
        progress.counting(files, bytes, directories);
        while let Some((cursor, directory)) = stack.last_mut() {
            crate::error::check_cancelled()?;
            let Some(name) = cursor
                .next_name()
                .map_err(|e| HostError::io("reading a mount directory", e))?
            else {
                stack.pop();
                continue;
            };
            let meta = directory
                .lstat(&name)
                .map_err(|e| HostError::io("measuring a mount entry", e))?;
            let child: Option<Dir> = if meta.is_dir() {
                let child = directory
                    .open_child(&name)
                    .map_err(|e| HostError::io("opening a mount directory", e))?;
                if child
                    .stat_self()
                    .map_err(|e| HostError::io("checking a mount directory", e))?
                    .identity()
                    != meta.identity()
                {
                    return Err(HostError::Unsupported(
                        "a mount directory changed while it was measured".into(),
                    ));
                }
                directories = directories.checked_add(1).ok_or_else(|| {
                    HostError::Unsupported("the mount directory count cannot be represented".into())
                })?;
                Some(child)
            } else {
                if meta.is_file() {
                    files = files.checked_add(1).ok_or_else(|| {
                        HostError::Unsupported("the mount file count cannot be represented".into())
                    })?;
                    bytes = bytes
                        .checked_add(i64::try_from(meta.size).map_err(|_| {
                            HostError::Unsupported(
                                "the mount byte count cannot be represented".into(),
                            )
                        })?)
                        .ok_or_else(|| {
                            HostError::Unsupported(
                                "the mount byte count cannot be represented".into(),
                            )
                        })?;
                }
                None
            };
            if let Some(child) = child {
                stack.push((
                    child
                        .names_cursor()
                        .map_err(|e| HostError::io("reading a mount directory", e))?,
                    child,
                ));
            }
            if reported.elapsed() >= crate::snapshot::PROGRESS_INTERVAL {
                progress.counting(files, bytes, directories);
                reported = Instant::now();
            }
        }
        progress.counting(files, bytes, directories);
        Ok(VolumeMeasure {
            file_count: files,
            total_bytes: bytes,
            file_states: None,
        })
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "mount_transfer/export_tests.rs"]
mod export_tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "mount_transfer/import_tests.rs"]
mod import_tests;
