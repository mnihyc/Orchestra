//! One-run connector files, created through a verified memory-backed directory.
//!
//! The guard may write these only after authenticating the retained supervisor
//! and proving that no untrusted workload processes are running.

#![cfg(target_os = "linux")]

use std::fs::File;
use std::io::Write;
use std::os::fd::OwnedFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use nix::sys::statfs::{TMPFS_MAGIC, fstatfs};
use orch_driver_contract::driver::DriverError;
use rustix::fs::{
    AtFlags, CWD, Gid, Mode, OFlags, Uid, fchmod, fchown, openat, renameat, unlinkat,
};

use crate::launch::LaunchFiles;

/// Ample room for the authority and mount list while bounding the one-run transfer.
pub(crate) const MAX_LAUNCH_BYTES: usize = 8 * 1024 * 1024;
const CA: &str = "ca.pem";
const ENROLLMENT: &str = "enroll";
const ENVIRONMENT: &str = "environment.json";
const MOUNTS: &str = "mounts.json";
const SECRET_MODE: Mode = Mode::RUSR.union(Mode::WUSR);

fn unavailable() -> DriverError {
    DriverError::Unavailable("the private connector launch files could not be prepared".into())
}

/// An open directory descriptor pins the memory-backed mount used for one run's files.
pub(crate) struct RunDirectory {
    fd: OwnedFd,
}

impl RunDirectory {
    fn descriptor(path: &Path) -> Result<OwnedFd, DriverError> {
        if !path.is_absolute() {
            return Err(DriverError::InvalidSpec(
                "the connector launch directory must be absolute".into(),
            ));
        }
        openat(
            CWD,
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| unavailable())
    }

    /// Refuses an ordinary persistent filesystem even if a Pod mount was configured as memory-backed.
    pub(crate) fn open(path: &Path) -> Result<Self, DriverError> {
        let fd = Self::descriptor(path)?;
        if fstatfs(&fd).map_err(|_| unavailable())?.filesystem_type() != TMPFS_MAGIC {
            return Err(DriverError::Unavailable(
                "the private connector launch directory is not memory-backed".into(),
            ));
        }
        Ok(Self { fd })
    }

    pub(crate) fn as_fd(&self) -> &OwnedFd {
        &self.fd
    }

    #[cfg(test)]
    pub(crate) fn test_filesystem(path: &Path) -> Result<Self, DriverError> {
        Ok(Self {
            fd: Self::descriptor(path)?,
        })
    }

    fn replace(
        &self,
        name: &'static str,
        bytes: &[u8],
        uid: Uid,
        gid: Gid,
    ) -> Result<(), DriverError> {
        let temporary = format!(".launch-{}", orch_types::new_id());
        let fd = openat(
            &self.fd,
            &temporary,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            SECRET_MODE,
        )
        .map_err(|_| unavailable())?;
        let result = (|| {
            // The guard owns the new file now, but need not have FOWNER after
            // assigning it to a non-root workload. Set the exact mode first.
            fchmod(&fd, SECRET_MODE).map_err(|_| unavailable())?;
            fchown(&fd, Some(uid), Some(gid)).map_err(|_| unavailable())?;
            let mut file = File::from(fd);
            let meta = file.metadata().map_err(|_| unavailable())?;
            if !meta.is_file()
                || meta.uid() != uid.as_raw()
                || meta.gid() != gid.as_raw()
                || meta.mode() & 0o7777 != 0o600
                || meta.nlink() != 1
            {
                return Err(unavailable());
            }
            file.write_all(bytes).map_err(|_| unavailable())?;
            drop(file);
            renameat(&self.fd, &temporary, &self.fd, name).map_err(|_| unavailable())
        })();
        if result.is_err() {
            let _ = unlinkat(&self.fd, &temporary, AtFlags::empty());
        }
        result
    }

    /// Writes a fresh set while stopped; the enrollment token is never a Pod environment value.
    pub(crate) fn prepare(
        &self,
        files: &LaunchFiles,
        uid: u32,
        gid: u32,
    ) -> Result<(), DriverError> {
        if uid == u32::MAX || gid == u32::MAX {
            return Err(DriverError::InvalidSpec(
                "the connector identity is invalid".into(),
            ));
        }
        let uid = Uid::from_raw(uid);
        let gid = Gid::from_raw(gid);
        // Remove the previous run's private values before preparing the next one.
        self.replace(ENROLLMENT, b"", uid, gid)?;
        self.replace(ENVIRONMENT, b"{}", uid, gid)?;
        let total = files
            .ca
            .len()
            .checked_add(files.enrollment.expose().len())
            .and_then(|size| size.checked_add(files.environment.expose().len()))
            .and_then(|size| size.checked_add(files.mounts.as_ref().map_or(0, String::len)))
            .ok_or_else(unavailable)?;
        if total > MAX_LAUNCH_BYTES {
            return Err(DriverError::InvalidSpec(
                "the connector launch files are too large".into(),
            ));
        }
        self.replace(CA, files.ca.as_bytes(), uid, gid)?;
        self.replace(ENVIRONMENT, files.environment.expose().as_bytes(), uid, gid)?;
        if let Some(mounts) = &files.mounts {
            self.replace(MOUNTS, mounts.as_bytes(), uid, gid)?;
        } else {
            match unlinkat(&self.fd, MOUNTS, AtFlags::empty()) {
                Ok(()) | Err(rustix::io::Errno::NOENT) => {}
                Err(_) => return Err(unavailable()),
            }
        }
        self.replace(ENROLLMENT, files.enrollment.expose().as_bytes(), uid, gid)
    }

    /// Clears the run's private values after the guard has stopped every workload process.
    pub(crate) fn clear_private(&self, uid: u32, gid: u32) -> Result<(), DriverError> {
        if uid == u32::MAX || gid == u32::MAX {
            return Err(DriverError::InvalidSpec(
                "the connector identity is invalid".into(),
            ));
        }
        let uid = Uid::from_raw(uid);
        let gid = Gid::from_raw(gid);
        self.replace(ENROLLMENT, b"", uid, gid)?;
        self.replace(ENVIRONMENT, b"{}", uid, gid)
    }

    /// Clears the one-use token after enrollment without changing the retained environment.
    pub(crate) fn clear_enrollment(&self, uid: u32, gid: u32) -> Result<(), DriverError> {
        if uid == u32::MAX || gid == u32::MAX {
            return Err(DriverError::InvalidSpec(
                "the connector identity is invalid".into(),
            ));
        }
        self.replace(ENROLLMENT, b"", Uid::from_raw(uid), Gid::from_raw(gid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_plugin_contract::Secret;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn files() -> LaunchFiles {
        LaunchFiles {
            ca: "test-authority".into(),
            enrollment: Secret::new("synthetic-once"),
            environment: Secret::new("{\"TOKEN\":\"synthetic-env-secret\"}"),
            mounts: Some("[]".into()),
        }
    }

    #[test]
    fn fixed_files_are_private_and_the_token_is_empty_after_a_stop() {
        let temp = tempfile::tempdir().unwrap();
        // This exercises safe file operations on an ordinary test filesystem; open() separately enforces tmpfs.
        let dir = RunDirectory {
            fd: RunDirectory::descriptor(temp.path()).unwrap(),
        };
        let uid = rustix::process::geteuid().as_raw();
        let gid = rustix::process::getegid().as_raw();
        dir.prepare(&files(), uid, gid).unwrap();
        assert_eq!(
            fs::read(temp.path().join(ENROLLMENT)).unwrap(),
            b"synthetic-once"
        );
        assert_eq!(
            fs::metadata(temp.path().join(ENROLLMENT))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(!format!("{:?}", files()).contains("synthetic-once"));
        assert!(!format!("{:?}", files()).contains("synthetic-env-secret"));
        assert_eq!(
            fs::read(temp.path().join(ENVIRONMENT)).unwrap(),
            b"{\"TOKEN\":\"synthetic-env-secret\"}"
        );
        dir.clear_private(uid, gid).unwrap();
        assert_eq!(fs::read(temp.path().join(ENROLLMENT)).unwrap(), b"");
        assert_eq!(fs::read(temp.path().join(ENVIRONMENT)).unwrap(), b"{}");
    }

    #[test]
    fn a_supplied_symlink_is_replaced_without_touching_its_target() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("unrelated");
        fs::write(&target, b"unchanged").unwrap();
        symlink(&target, temp.path().join(ENROLLMENT)).unwrap();
        let dir = RunDirectory {
            fd: RunDirectory::descriptor(temp.path()).unwrap(),
        };
        dir.prepare(
            &files(),
            rustix::process::geteuid().as_raw(),
            rustix::process::getegid().as_raw(),
        )
        .unwrap();
        assert_eq!(fs::read(target).unwrap(), b"unchanged");
        assert!(
            !fs::symlink_metadata(temp.path().join(ENROLLMENT))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn the_directory_must_be_memory_backed_and_the_identity_must_be_valid() {
        let temp = tempfile::tempdir().unwrap();
        let fd = RunDirectory::descriptor(temp.path()).unwrap();
        let memory_backed = fstatfs(&fd).unwrap().filesystem_type() == TMPFS_MAGIC;
        assert_eq!(RunDirectory::open(temp.path()).is_ok(), memory_backed);
        let dir = RunDirectory { fd };
        assert!(
            dir.prepare(&files(), u32::MAX, rustix::process::getegid().as_raw())
                .is_err()
        );
        assert!(!temp.path().join(ENROLLMENT).exists());
    }
}
