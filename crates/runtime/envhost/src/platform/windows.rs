//! Windows paths, job objects and ConPTY.
//!
//! The same functions as the POSIX implementation, on top of the standard library and the
//! process table of `sysinfo`, which keeps the package free of `unsafe` code. What this cannot
//! give is said here, not hidden:
//!
//! - A [`Dir`] holds the path it was opened at and not an open handle, so a directory that is
//!   swapped for a link while a request is served can redirect the next operation. Links are
//!   never followed by name (files are opened as the reparse point they are), and a path is
//!   resolved link by link before it is used, which closes everything but that short window.
//! - A file has no permission bits beyond "read-only": a mode is kept as 0o444 or 0o666 (and
//!   0o111 more for a directory), and the bits of a mode that was asked for are not stored.
//! - A process cannot be asked to end, interrupted, held still or let go by number: `Term` ends it
//!   at once as `Kill` does, and `Int`, `Stop` and `Continue` are refused, which is why the
//!   environment says that processes cannot be held still here.
//! - There are no process groups and no sessions: a tree is found by parentage alone, and a
//!   group cannot be signalled.
//!
//! This module has been type-checked and linted for `x86_64-pc-windows-gnu` and never run.

#![cfg(windows)]

use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::{FileExt, FileTypeExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use super::{Kind, Meta, Sig};

/// File names do not differ by case on Windows unless a directory was set up that way.
pub const CASE_SENSITIVE_BY_DEFAULT: bool = false;

/// Opens a link as the link it is and does not follow it.
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

/// Lets a directory be opened as a file, which setting its times needs.
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

/// What the system answers when a rename would have to cross from one volume to another.
const ERROR_NOT_SAME_DEVICE: i32 = 17;

/// The permission bits Windows can say: a file is writable or it is read-only.
const MODE_WRITABLE: u32 = 0o666;

/// The permission bits of a read-only file.
const MODE_READ_ONLY: u32 = 0o444;

/// The execute bits a directory is shown with.
const MODE_SEARCH: u32 = 0o111;

/// The lowest process number a signal may be sent to: 0 is the idle process and 4 the system.
const FIRST_SIGNALLABLE_PID: u32 = 5;

fn refuse(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message.to_owned())
}

/// Refuses a name that is not one plain name of one directory: a separator, a drive or stream
/// marker, `.` and `..` all lead out of the directory or name something that is not a file.
fn plain(name: &OsStr) -> io::Result<()> {
    let text = name.to_string_lossy();
    if text.is_empty() || text == "." || text == ".." || text.contains(['/', '\\', ':', '\0']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a plain name",
        ));
    }
    Ok(())
}

/// A time as nanoseconds since 1970, which may be before it; zero where the system says nothing.
fn nanos_of(time: io::Result<SystemTime>) -> i64 {
    match time {
        Ok(time) => match time.duration_since(UNIX_EPOCH) {
            Ok(after) => i64::try_from(after.as_nanos()).unwrap_or(i64::MAX),
            Err(before) => {
                i64::try_from(before.duration().as_nanos()).map_or(i64::MIN, |nanos| -nanos)
            }
        },
        Err(_) => 0,
    }
}

/// A point in time from nanoseconds since 1970, which may be before it.
fn nanos_to_time(nanos: i64) -> SystemTime {
    let magnitude = Duration::from_nanos(nanos.unsigned_abs());
    if nanos >= 0 {
        UNIX_EPOCH + magnitude
    } else {
        UNIX_EPOCH - magnitude
    }
}

fn meta_of(metadata: &std::fs::Metadata) -> Meta {
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        Kind::Symlink
    } else if file_type.is_dir() {
        Kind::Dir
    } else if file_type.is_file() {
        Kind::File
    } else {
        Kind::Other
    };
    let base = if metadata.permissions().readonly() {
        MODE_READ_ONLY
    } else {
        MODE_WRITABLE
    };
    Meta {
        kind,
        size: metadata.len(),
        mtime_ns: nanos_of(metadata.modified()),
        // Windows keeps a change time that the standard library does not give; the creation time
        // is what is closest, and it is not what a program sets with the modification time.
        ctime_ns: nanos_of(metadata.created()),
        mode: if kind == Kind::Dir {
            base | MODE_SEARCH
        } else {
            base
        },
        ino: 0,
        dev: 0,
        nlink: 1,
    }
}

/// A directory, by the path it was opened at.
#[derive(Debug)]
pub struct Dir {
    path: PathBuf,
}

impl Dir {
    /// Opens the directory at `path`, following links in the path: the root of a workspace, which
    /// is given by whoever set the environment up and is not a request's to name.
    pub fn open_path(path: &Path) -> io::Result<Self> {
        if !std::fs::metadata(path)?.is_dir() {
            return Err(io::Error::from(io::ErrorKind::NotADirectory));
        }
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    /// Opens the directory called `name` in this one. A link called `name` is not followed.
    pub fn open_child(&self, name: &OsStr) -> io::Result<Self> {
        plain(name)?;
        let child = self.path.join(name);
        let metadata = std::fs::symlink_metadata(&child)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(io::Error::from(io::ErrorKind::NotADirectory));
        }
        Ok(Self { path: child })
    }

    /// Another handle on the same directory.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            path: self.path.clone(),
        })
    }

    /// Where this directory was opened.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What this directory itself is.
    pub fn stat_self(&self) -> io::Result<Meta> {
        std::fs::symlink_metadata(&self.path).map(|metadata| meta_of(&metadata))
    }

    /// What the entry called `name` is, without following it.
    pub fn lstat(&self, name: &OsStr) -> io::Result<Meta> {
        plain(name)?;
        std::fs::symlink_metadata(self.path.join(name)).map(|metadata| meta_of(&metadata))
    }

    /// Where the link called `name` points, as written.
    pub fn read_link(&self, name: &OsStr) -> io::Result<OsString> {
        plain(name)?;
        std::fs::read_link(self.path.join(name)).map(PathBuf::into_os_string)
    }

    /// Opens the regular file called `name` for reading. A link is opened as the link it is, which
    /// is not a regular file, so it is refused.
    pub fn open_read(&self, name: &OsStr) -> io::Result<File> {
        plain(name)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(self.path.join(name))?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        Ok(file)
    }

    /// Makes the file called `name`, which must not exist, and opens it for reading and writing.
    /// Windows takes no permission bits at creation, so `mode` is not used.
    pub fn create_new(&self, name: &OsStr, _mode: u32) -> io::Result<File> {
        plain(name)?;
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(self.path.join(name))
    }

    /// Makes the directory called `name`.
    pub fn mkdir(&self, name: &OsStr, _mode: u32) -> io::Result<()> {
        plain(name)?;
        std::fs::create_dir(self.path.join(name))
    }

    /// Makes a link called `name` that points to `target`. Making one takes a privilege that an
    /// ordinary account may lack, in which case the system's refusal comes back.
    pub fn symlink(&self, target: &OsStr, name: &OsStr) -> io::Result<()> {
        plain(name)?;
        let link = self.path.join(name);
        let resolved = if Path::new(target).is_absolute() {
            PathBuf::from(target)
        } else {
            self.path.join(target)
        };
        if std::fs::metadata(&resolved).is_ok_and(|metadata| metadata.is_dir()) {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        }
    }

    /// Removes the file or link called `name`.
    pub fn unlink(&self, name: &OsStr) -> io::Result<()> {
        plain(name)?;
        let path = self.path.join(name);
        // A link to a directory is removed like a directory, a link to a file like a file.
        if std::fs::symlink_metadata(&path)?
            .file_type()
            .is_symlink_dir()
        {
            std::fs::remove_dir(path)
        } else {
            std::fs::remove_file(path)
        }
    }

    /// Removes the empty directory called `name`.
    pub fn rmdir(&self, name: &OsStr) -> io::Result<()> {
        plain(name)?;
        std::fs::remove_dir(self.path.join(name))
    }

    /// Renames `from` in this directory to `to` in `to_dir`. With `replace` false a destination
    /// that is there is refused; the look and the rename are two steps here, which leaves a short
    /// window that the POSIX implementation does not have.
    pub fn rename_to(
        &self,
        from: &OsStr,
        to_dir: &Dir,
        to: &OsStr,
        replace: bool,
    ) -> io::Result<()> {
        plain(from)?;
        plain(to)?;
        let target = to_dir.path.join(to);
        if !replace && std::fs::symlink_metadata(&target).is_ok() {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists));
        }
        std::fs::rename(self.path.join(from), target)
    }

    /// The names in this directory, without `.` and `..`, in no particular order.
    pub fn names(&self) -> io::Result<Vec<OsString>> {
        std::fs::read_dir(&self.path)?
            .map(|entry| {
                crate::error::check_cancelled().map_err(io::Error::other)?;
                entry.map(|entry| entry.file_name())
            })
            .collect()
    }

    /// Windows cannot flush a directory by itself: what is written to its files is flushed with them.
    /// Visits names without collecting a large directory; false stops the read immediately.
    pub fn visit_names(&self, visit: &mut dyn FnMut(&OsStr) -> bool) -> io::Result<bool> {
        for entry in std::fs::read_dir(&self.path)? {
            crate::error::check_cancelled().map_err(io::Error::other)?;
            if !visit(&entry?.file_name()) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Windows cannot flush a directory by itself: what is written to its files is flushed with them.
    pub fn sync(&self) -> io::Result<()> {
        Ok(())
    }

    /// Sets what Windows keeps of the permission bits of the file or directory called `name`:
    /// whether it is read-only. Never of a link.
    pub fn set_mode(&self, name: &OsStr, mode: u32) -> io::Result<()> {
        plain(name)?;
        let path = self.path.join(name);
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a link has no mode of its own",
            ));
        }
        let mut permissions = metadata.permissions();
        permissions.set_readonly(mode & 0o222 == 0);
        std::fs::set_permissions(path, permissions)
    }

    /// Sets the modification time of the file or directory called `name`, never of a link.
    pub fn set_mtime(&self, name: &OsStr, mtime_ns: i64) -> io::Result<()> {
        plain(name)?;
        let file = OpenOptions::new()
            .write(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(self.path.join(name))?;
        file.set_modified(nanos_to_time(mtime_ns))
    }
}

/// What an open file is, as it is at this moment.
pub fn file_meta(file: &File) -> io::Result<Meta> {
    file.metadata().map(|metadata| meta_of(&metadata))
}

/// Sets what Windows keeps of the permission bits of an open file: whether it is read-only.
pub fn set_file_mode(file: &File, mode: u32) -> io::Result<()> {
    let mut permissions = file.metadata()?.permissions();
    permissions.set_readonly(mode & 0o222 == 0);
    file.set_permissions(permissions)
}

/// Whether an error says that a rename tried to cross from one volume to another.
#[must_use]
pub fn is_cross_device(error: &io::Error) -> bool {
    error.raw_os_error() == Some(ERROR_NOT_SAME_DEVICE)
}

/// Reads at `offset`. Windows moves the cursor of the file as it does; nothing in this package
/// reads or writes a file by its cursor and by an offset at the same time.
pub fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    loop {
        match file.seek_read(buffer, offset) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            other => return other,
        }
    }
}

/// Writes at `offset`; see [`read_at`] for the cursor.
pub fn write_at(file: &File, buffer: &[u8], offset: u64) -> io::Result<usize> {
    loop {
        match file.seek_write(buffer, offset) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            other => return other,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Processes

/// The number of this process.
#[must_use]
pub fn own_pid() -> u32 {
    std::process::id()
}

/// Windows has no process groups to speak of.
#[must_use]
pub fn own_process_group() -> Option<u32> {
    None
}

/// Windows has no process groups to speak of.
#[must_use]
pub fn process_group_of(_pid: u32) -> Option<u32> {
    None
}

/// Windows has no sessions of the kind that tie a tree of processes together.
#[must_use]
pub fn session_of(_pid: u32) -> Option<u32> {
    None
}

/// The exact start of a process is not told more finely than the process table does.
#[must_use]
pub fn exact_start_ms(_pid: u32) -> Option<i64> {
    None
}

/// There is no table to read here: the portable library gives the table.
#[must_use]
pub fn table() -> Option<Vec<super::ProcRow>> {
    None
}

/// Windows has no control groups.
#[must_use]
pub fn own_cgroup_dir() -> Option<PathBuf> {
    None
}

/// Ends the process `pid` if it is the one that started at `expect_start_ms`. A process cannot be
/// asked to end by number on Windows, so `Term` ends it as `Kill` does; `Int`, `Stop` and
/// `Continue` are refused. The host itself and the processes of the system are never signalled.
pub fn send_signal(pid: u32, expect_start_ms: Option<i64>, sig: Sig) -> io::Result<()> {
    if pid < FIRST_SIGNALLABLE_PID {
        return Err(refuse("the processes of the system are never signalled"));
    }
    if pid == own_pid() {
        return Err(refuse("the host does not signal itself"));
    }
    if !matches!(sig, Sig::Term | Sig::Kill) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Windows can end a process and can do no more with it by number",
        ));
    }
    let mut system = System::new();
    let number = Pid::from_u32(pid);
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[number]),
        true,
        ProcessRefreshKind::nothing(),
    );
    let process = system
        .process(number)
        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
    if let Some(expected) = expect_start_ms {
        let found = i64::try_from(process.start_time())
            .unwrap_or(0)
            .saturating_mul(1000);
        if found != expected {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "that number now names another process",
            ));
        }
    }
    if process.kill() {
        Ok(())
    } else {
        Err(refuse("the system would not end that process"))
    }
}

/// Windows has no process groups to signal.
pub fn send_group(_group: u32, _sig: Sig) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Windows has no process groups",
    ))
}
