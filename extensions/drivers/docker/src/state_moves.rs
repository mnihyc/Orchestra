//! A stopped unit's files below one directory of its state, taken out as an archive and put into another unit.
//!
//! When a runtime goes to another host (a provider's runtime whose host is changed), what it keeps below one directory
//! of its state goes with it: the old unit, stopped so that its files stand still, gives the files of that directory
//! as the daemon archives a container's files (owners, modes and times kept), renamed to begin with the directory's
//! path inside the state; the new unit, before it first starts, takes them into its state. Both go through the
//! daemon's archive of a container: no helper container and no image is needed, and a unit's volume is only ever
//! reached through the unit's own container.

use std::path::Path;

use bollard::query_parameters::DownloadFromContainerOptionsBuilder;
use futures::StreamExt;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::units::UnitRef;
use tokio::io::AsyncWriteExt;

use crate::client::{map_error, status_of};
use crate::driver::DockerDriver;
use crate::spec::layout;

/// The largest archive a unit's files are moved in: 512 MiB. A sign-in and a harness's settings are kilobytes, and
/// the conversations a harness keeps rarely some hundreds of mebibytes; the archive is handed to the daemon from
/// memory, so the bound keeps a large home from taking the node's memory, and a move that would pass it fails before
/// the new unit first starts, the old one kept as it was.
pub const MOVE_ARCHIVE_MAX_BYTES: u64 = 512 * 1024 * 1024;

fn io(error: &std::io::Error) -> DriverError {
    DriverError::Internal(format!(
        "a moving unit's archive could not be written: {error}"
    ))
}

fn too_large() -> DriverError {
    DriverError::Unsupported(format!(
        "the files to move are larger than {} MiB",
        MOVE_ARCHIVE_MAX_BYTES / (1024 * 1024)
    ))
}

/// Copies the tar archive at `from` to `into` with each name put below `parent`; owners, modes, times and links are
/// kept as the archive has them, and a hard link names its target below `parent` too.
pub fn rebase(from: &Path, into: &Path, parent: &Path) -> std::io::Result<()> {
    let mut input = tar::Archive::new(std::fs::File::open(from)?);
    let mut output = tar::Builder::new(std::fs::File::create(into)?);
    for entry in input.entries()? {
        let mut entry = entry?;
        let name = entry.path()?.into_owned();
        let mut header = entry.header().clone();
        let renamed = parent.join(&name);
        match header.entry_type() {
            tar::EntryType::Symlink | tar::EntryType::Link => {
                let target = entry
                    .link_name()?
                    .map(std::borrow::Cow::into_owned)
                    .unwrap_or_default();
                let target = if header.entry_type() == tar::EntryType::Link {
                    parent.join(target)
                } else {
                    target
                };
                output.append_link(&mut header, &renamed, &target)?;
            }
            _ => output.append_data(&mut header, &renamed, &mut entry)?,
        }
    }
    output.into_inner()?.sync_all()
}

impl DockerDriver {
    /// Writes the files a stopped unit keeps below `path` of its state as a tar archive to `into`, each name
    /// beginning with `path`; `false` where it keeps nothing there. See the node's `DriverExtras::export_state`.
    pub async fn export_state(
        &self,
        unit: &UnitRef,
        path: &str,
        into: &Path,
    ) -> Result<bool, DriverError> {
        let _held = self.locks.lock(&unit.name).await;
        let container = self.container_of(unit).await?.ok_or_else(|| {
            DriverError::NotFound(format!("there is no unit {} of this driver", unit.name))
        })?;
        if container.state.as_ref().and_then(|state| state.running) == Some(true) {
            return Err(DriverError::Unavailable(
                "the unit runs; its files are taken once it has stopped".to_owned(),
            ));
        }
        let id = container.id.clone().unwrap_or_else(|| unit.name.clone());
        let source = format!("{}/{path}", layout::STATE_DIR);
        let raw = into.with_extension("daemon.tar");
        let options = DownloadFromContainerOptionsBuilder::default()
            .path(&source)
            .build();
        let mut stream = self
            .daemon
            .docker()
            .download_from_container(&id, Some(options));
        let mut file = tokio::fs::File::create(&raw).await.map_err(|e| io(&e))?;
        let mut written: u64 = 0;
        let mut outcome = Ok(true);
        while let Some(piece) = stream.next().await {
            match piece {
                Ok(bytes) => {
                    written =
                        written.saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
                    if written > MOVE_ARCHIVE_MAX_BYTES {
                        outcome = Err(too_large());
                        break;
                    }
                    if let Err(error) = file.write_all(&bytes).await {
                        outcome = Err(io(&error));
                        break;
                    }
                }
                // Nothing kept there: nothing to move.
                Err(error) if status_of(&error) == Some(404) => {
                    outcome = Ok(false);
                    break;
                }
                Err(error) => {
                    outcome = Err(map_error(error));
                    break;
                }
            }
        }
        let flushed = file.flush().await;
        drop(file);
        if matches!(outcome, Ok(true)) {
            flushed.map_err(|e| io(&e))?;
            let parent = Path::new(path)
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            let (from, to) = (raw.clone(), into.to_path_buf());
            let rebased = tokio::task::spawn_blocking(move || rebase(&from, &to, &parent))
                .await
                .map_err(|error| DriverError::Internal(error.to_string()))?;
            if let Err(error) = rebased {
                outcome = Err(io(&error));
            }
        }
        let _ = tokio::fs::remove_file(&raw).await;
        outcome
    }

    /// Puts the files of a tar archive [`DockerDriver::export_state`] wrote into a unit's state, with their owners and
    /// modes, before the unit first starts.
    pub async fn import_state(&self, unit: &UnitRef, from: &Path) -> Result<(), DriverError> {
        let _held = self.locks.lock(&unit.name).await;
        let container = self.container_of(unit).await?.ok_or_else(|| {
            DriverError::NotFound(format!("there is no unit {} of this driver", unit.name))
        })?;
        let size = tokio::fs::metadata(from).await.map_err(|e| io(&e))?.len();
        if size > MOVE_ARCHIVE_MAX_BYTES {
            return Err(too_large());
        }
        let archive = tokio::fs::read(from).await.map_err(|e| io(&e))?;
        let id = container.id.clone().unwrap_or_else(|| unit.name.clone());
        self.daemon
            .put_archive(&id, layout::STATE_DIR, archive)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moved_archive_keeps_owners_modes_and_links_with_every_name_below_the_directory_it_came_from()
     {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from.tar");
        let into = dir.path().join("into.tar");
        {
            let mut builder = tar::Builder::new(std::fs::File::create(&from).unwrap());
            let mut directory = tar::Header::new_gnu();
            directory.set_entry_type(tar::EntryType::Directory);
            directory.set_mode(0o700);
            directory.set_uid(1000);
            directory.set_gid(1000);
            directory.set_size(0);
            builder
                .append_data(&mut directory, "provider-1", std::io::empty())
                .unwrap();
            let mut file = tar::Header::new_gnu();
            file.set_mode(0o600);
            file.set_uid(1000);
            file.set_gid(1000);
            file.set_size(5);
            builder
                .append_data(&mut file, "provider-1/agent/auth.json", &b"token"[..])
                .unwrap();
            let mut link = tar::Header::new_gnu();
            link.set_entry_type(tar::EntryType::Symlink);
            link.set_mode(0o777);
            link.set_uid(1000);
            link.set_gid(1000);
            link.set_size(0);
            builder
                .append_link(&mut link, "provider-1/current", "agent")
                .unwrap();
            builder.into_inner().unwrap();
        }
        rebase(&from, &into, Path::new("harness")).unwrap();
        let mut read = tar::Archive::new(std::fs::File::open(&into).unwrap());
        let seen: Vec<(String, u32, u64, u64, Option<String>)> = read
            .entries()
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry
                        .path()
                        .unwrap()
                        .to_string_lossy()
                        .trim_end_matches('/')
                        .to_owned(),
                    entry.header().mode().unwrap(),
                    entry.header().uid().unwrap(),
                    entry.header().gid().unwrap(),
                    entry
                        .link_name()
                        .unwrap()
                        .map(|target| target.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(
            seen[0],
            ("harness/provider-1".to_owned(), 0o700, 1000, 1000, None)
        );
        assert_eq!(
            seen[1],
            (
                "harness/provider-1/agent/auth.json".to_owned(),
                0o600,
                1000,
                1000,
                None
            )
        );
        assert_eq!(
            seen[2],
            (
                "harness/provider-1/current".to_owned(),
                0o777,
                1000,
                1000,
                Some("agent".to_owned())
            ),
            "a symbolic link keeps its metadata and target as they were"
        );
    }
}
