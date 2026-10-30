//! File operations relative to a directory handle, shared by Linux and macOS.
//!
//! A [`Dir`] holds an open directory. Everything below is a single name inside it: a name that
//! holds a path separator, or is `.` or `..`, is refused before the system is asked, and no
//! operation follows a link in the name it is given. A caller that walks a path therefore opens
//! one directory after another and decides for itself what a link inside the path means, and a
//! directory that is swapped for a link after it was opened changes nothing about where an
//! operation on the handle lands.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use rustix::fs::{AtFlags, CWD, FileType, Mode, OFlags, RawMode, RenameFlags};

use super::{Kind, Meta, Sig, unix_nanos};

/// A directory that is held open.
#[derive(Debug)]
pub struct Dir {
    fd: OwnedFd,
    path: PathBuf,
}

fn io_error(errno: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(errno.raw_os_error())
}

/// Refuses a name that is not one plain name of one directory.
fn plain(name: &OsStr) -> io::Result<()> {
    let bytes = name.as_bytes();
    if bytes.is_empty()
        || bytes == b"."
        || bytes == b".."
        || bytes.contains(&b'/')
        || bytes.contains(&0)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a plain name",
        ));
    }
    Ok(())
}

#[allow(
    clippy::unnecessary_cast,
    clippy::useless_conversion,
    reason = "the fields of a stat have different widths on different Unix systems (device numbers are 32 bits on macOS, link counts 16), so each is widened to the width a Meta holds"
)]
fn meta_of(stat: &rustix::fs::Stat) -> Meta {
    let kind = match FileType::from_raw_mode(stat.st_mode as RawMode) {
        FileType::RegularFile => Kind::File,
        FileType::Directory => Kind::Dir,
        FileType::Symlink => Kind::Symlink,
        _ => Kind::Other,
    };
    Meta {
        kind,
        size: u64::try_from(stat.st_size).unwrap_or(0),
        mtime_ns: unix_nanos(stat.st_mtime as i64, stat.st_mtime_nsec as i64),
        ctime_ns: unix_nanos(stat.st_ctime as i64, stat.st_ctime_nsec as i64),
        mode: u32::from(stat.st_mode as RawMode) & 0o7777,
        ino: stat.st_ino as u64,
        dev: stat.st_dev as u64,
        nlink: stat.st_nlink as u64,
    }
}

impl Dir {
    /// Opens an existing directory beneath an already resolved root without following new links.
    #[cfg(target_os = "linux")]
    pub(crate) fn open_beneath(root: &Path, relative: &Path, path: &Path) -> io::Result<Self> {
        use rustix::fs::{ResolveFlags, openat2};
        let base = openat2(
            CWD,
            root,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(io_error)?;
        let relative = if relative.as_os_str().is_empty() {
            Path::new(".")
        } else {
            relative
        };
        let fd = openat2(
            &base,
            relative,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(io_error)?;
        Ok(Self {
            fd,
            path: path.to_path_buf(),
        })
    }

    /// Opens a readable handle to this same held directory.
    #[cfg(target_os = "linux")]
    pub(crate) fn readable(self) -> io::Result<Self> {
        let fd = rustix::fs::openat(
            &self.fd,
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io_error)?;
        Ok(Self {
            fd,
            path: self.path,
        })
    }

    /// Checks access on the held directory rather than reopening its pathname.
    #[cfg(target_os = "linux")]
    pub(crate) fn require_writable(&self) -> io::Result<()> {
        rustix::fs::accessat(
            &self.fd,
            ".",
            rustix::fs::Access::WRITE_OK | rustix::fs::Access::EXEC_OK,
            AtFlags::EACCESS,
        )
        .map_err(io_error)
    }

    /// An external process can enter this directory while this handle remains open.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn process_path(&self) -> PathBuf {
        use std::os::fd::AsRawFd;
        PathBuf::from(format!(
            "/proc/{}/fd/{}",
            std::process::id(),
            self.fd.as_raw_fd()
        ))
    }

    /// Opens the directory at `path`, following links in the path: the root of a workspace, which
    /// is given by whoever set the environment up and is not a request's to name.
    pub fn open_path(path: &Path) -> io::Result<Self> {
        let fd = rustix::fs::openat(
            CWD,
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io_error)?;
        Ok(Self {
            fd,
            path: path.to_path_buf(),
        })
    }

    /// Opens the directory called `name` in this one. A link called `name` is not followed.
    pub fn open_child(&self, name: &OsStr) -> io::Result<Self> {
        plain(name)?;
        let fd = rustix::fs::openat(
            &self.fd,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io_error)?;
        Ok(Self {
            fd,
            path: self.path.join(name),
        })
    }

    /// Another handle on the same directory.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            fd: self.fd.try_clone()?,
            path: self.path.clone(),
        })
    }

    /// Where this directory was opened, for messages and for programs that need a working
    /// directory. It is not what operations on the handle go through.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What this directory itself is.
    pub fn stat_self(&self) -> io::Result<Meta> {
        rustix::fs::fstat(&self.fd)
            .map(|stat| meta_of(&stat))
            .map_err(io_error)
    }

    /// What the entry called `name` is, without following it.
    pub fn lstat(&self, name: &OsStr) -> io::Result<Meta> {
        plain(name)?;
        rustix::fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW)
            .map(|stat| meta_of(&stat))
            .map_err(io_error)
    }

    /// Where the link called `name` points, as written.
    pub fn read_link(&self, name: &OsStr) -> io::Result<OsString> {
        plain(name)?;
        let target = rustix::fs::readlinkat(&self.fd, name, Vec::new()).map_err(io_error)?;
        Ok(OsString::from_vec(target.into_bytes()))
    }

    /// Opens the regular file called `name` for reading. A link is not followed, and a pipe or a
    /// device is not waited on: whatever is opened is checked to be a regular file.
    pub fn open_read(&self, name: &OsStr) -> io::Result<File> {
        plain(name)?;
        let fd = rustix::fs::openat(
            &self.fd,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io_error)?;
        let file = File::from(fd);
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        Ok(file)
    }

    /// Makes the file called `name`, which must not exist, and opens it for reading and writing
    /// with the permissions `mode` (as the process's umask lets them through). It is read back to
    /// be hashed before it is put in place.
    pub fn create_new(&self, name: &OsStr, mode: u32) -> io::Result<File> {
        plain(name)?;
        let fd = rustix::fs::openat(
            &self.fd,
            name,
            OFlags::RDWR
                | OFlags::CREATE
                | OFlags::EXCL
                | OFlags::NOFOLLOW
                | OFlags::NOCTTY
                | OFlags::CLOEXEC,
            Mode::from_bits_truncate(mode as RawMode),
        )
        .map_err(io_error)?;
        Ok(File::from(fd))
    }

    /// Makes the directory called `name`.
    pub fn mkdir(&self, name: &OsStr, mode: u32) -> io::Result<()> {
        plain(name)?;
        rustix::fs::mkdirat(&self.fd, name, Mode::from_bits_truncate(mode as RawMode))
            .map_err(io_error)
    }

    /// Makes a link called `name` that points to `target`.
    pub fn symlink(&self, target: &OsStr, name: &OsStr) -> io::Result<()> {
        plain(name)?;
        rustix::fs::symlinkat(target, &self.fd, name).map_err(io_error)
    }

    /// Removes the file or link called `name`.
    pub fn unlink(&self, name: &OsStr) -> io::Result<()> {
        plain(name)?;
        rustix::fs::unlinkat(&self.fd, name, AtFlags::empty()).map_err(io_error)
    }

    /// Removes the empty directory called `name`.
    pub fn rmdir(&self, name: &OsStr) -> io::Result<()> {
        plain(name)?;
        rustix::fs::unlinkat(&self.fd, name, AtFlags::REMOVEDIR).map_err(io_error)
    }

    /// Renames `from` in this directory to `to` in `to_dir`. With `replace` false the destination
    /// is never overwritten, and the check and the rename are one step where the system can do
    /// that.
    pub fn rename_to(
        &self,
        from: &OsStr,
        to_dir: &Dir,
        to: &OsStr,
        replace: bool,
    ) -> io::Result<()> {
        plain(from)?;
        plain(to)?;
        if replace {
            return rustix::fs::renameat(&self.fd, from, &to_dir.fd, to).map_err(io_error);
        }
        match rustix::fs::renameat_with(&self.fd, from, &to_dir.fd, to, RenameFlags::NOREPLACE) {
            Ok(()) => Ok(()),
            // A file system without the flag: look first, which leaves a short window.
            Err(
                rustix::io::Errno::INVAL | rustix::io::Errno::NOSYS | rustix::io::Errno::OPNOTSUPP,
            ) => match to_dir.lstat(to) {
                Ok(_) => Err(io::Error::from(io::ErrorKind::AlreadyExists)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    rustix::fs::renameat(&self.fd, from, &to_dir.fd, to).map_err(io_error)
                }
                Err(error) => Err(error),
            },
            Err(errno) => Err(io_error(errno)),
        }
    }

    /// The names in this directory, without `.` and `..`, in no particular order.
    pub fn names(&self) -> io::Result<Vec<OsString>> {
        let mut listing = rustix::fs::Dir::read_from(&self.fd).map_err(io_error)?;
        let mut names = Vec::new();
        while let Some(entry) = listing.read() {
            crate::error::check_cancelled().map_err(io::Error::other)?;
            let entry = entry.map_err(io_error)?;
            let name = entry.file_name().to_bytes();
            if name != b"." && name != b".." {
                names.push(OsString::from_vec(name.to_vec()));
            }
        }
        Ok(names)
    }

    /// Writes this directory's own changes to disk, where the system can.
    /// Visits names directly from the directory stream; false stops without collecting or sorting the rest.
    pub fn visit_names(&self, visit: &mut dyn FnMut(&OsStr) -> bool) -> io::Result<bool> {
        let mut listing = rustix::fs::Dir::read_from(&self.fd).map_err(io_error)?;
        while let Some(entry) = listing.read() {
            crate::error::check_cancelled().map_err(io::Error::other)?;
            let entry = entry.map_err(io_error)?;
            let name = entry.file_name().to_bytes();
            if name != b"." && name != b".." && !visit(OsStr::from_bytes(name)) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Writes this directory's own changes to disk, where the system can.
    pub fn sync(&self) -> io::Result<()> {
        rustix::fs::fsync(&self.fd).map_err(io_error)
    }

    /// Sets the permission bits of the file or directory called `name`, never of a link.
    pub fn set_mode(&self, name: &OsStr, mode: u32) -> io::Result<()> {
        plain(name)?;
        let fd = rustix::fs::openat(
            &self.fd,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io_error)?;
        rustix::fs::fchmod(&fd, Mode::from_bits_truncate(mode as RawMode)).map_err(io_error)
    }

    /// Sets the modification time of the file or directory called `name`, never of a link.
    pub fn set_mtime(&self, name: &OsStr, mtime_ns: i64) -> io::Result<()> {
        plain(name)?;
        let fd = rustix::fs::openat(
            &self.fd,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io_error)?;
        let at = nanos_to_time(mtime_ns);
        File::from(fd).set_modified(at)
    }
}

/// What an open file is, as it is at this moment.
pub fn file_meta(file: &File) -> io::Result<Meta> {
    rustix::fs::fstat(file)
        .map(|stat| meta_of(&stat))
        .map_err(io_error)
}

/// Sets the permission bits of an open file.
pub fn set_file_mode(file: &File, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(mode))
}

/// Whether an error says that a rename tried to cross from one file system to another.
#[must_use]
pub fn is_cross_device(error: &io::Error) -> bool {
    error.raw_os_error() == Some(rustix::io::Errno::XDEV.raw_os_error())
}

/// A point in time from nanoseconds since 1970, which may be before it.
fn nanos_to_time(nanos: i64) -> std::time::SystemTime {
    let magnitude = std::time::Duration::from_nanos(nanos.unsigned_abs());
    if nanos >= 0 {
        std::time::UNIX_EPOCH + magnitude
    } else {
        std::time::UNIX_EPOCH - magnitude
    }
}

/// Reads at `offset` without moving the cursor of `file`; interrupted calls are repeated.
pub fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    loop {
        match file.read_at(buffer, offset) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            other => return other,
        }
    }
}

/// Writes at `offset` without moving the cursor of `file`; interrupted calls are repeated.
pub fn write_at(file: &File, buffer: &[u8], offset: u64) -> io::Result<usize> {
    loop {
        match file.write_at(buffer, offset) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            other => return other,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Signals

fn rustix_signal_of(sig: Sig) -> rustix::process::Signal {
    match sig {
        Sig::Int => rustix::process::Signal::INT,
        Sig::Term => rustix::process::Signal::TERM,
        Sig::Kill => rustix::process::Signal::KILL,
        Sig::Stop => rustix::process::Signal::STOP,
        Sig::Continue => rustix::process::Signal::CONT,
    }
}

/// The signal as the system names it, for the process-handle calls of the Linux module.
#[cfg(target_os = "linux")]
pub(crate) fn rustix_signal(sig: Sig) -> rustix::process::Signal {
    rustix_signal_of(sig)
}

/// The number of this process.
#[must_use]
pub fn own_pid() -> u32 {
    std::process::id()
}

fn as_pid(pid: u32) -> Option<rustix::process::Pid> {
    i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
}

/// The process group of this process.
#[must_use]
pub fn own_process_group() -> Option<u32> {
    rustix::process::getpgid(None)
        .ok()
        .map(|group| group.as_raw_nonzero().get().unsigned_abs())
}

/// The process group `pid` is in, asked of the system call. Kernel threads and processes of
/// another namespace answer with 0, which the library this goes through does not expect, so
/// Linux reads the process table instead and this is for the systems that have no such file.
#[cfg_attr(
    target_os = "linux",
    allow(dead_code, reason = "Linux reads the process table")
)]
#[must_use]
pub(crate) fn group_by_call(pid: u32) -> Option<u32> {
    if pid <= 1 {
        return None;
    }
    let pid = as_pid(pid)?;
    rustix::process::getpgid(Some(pid))
        .ok()
        .map(|group| group.as_raw_nonzero().get().unsigned_abs())
}

/// The session `pid` is in, asked of the system call (see [`group_by_call`]).
#[cfg_attr(
    target_os = "linux",
    allow(dead_code, reason = "Linux reads the process table")
)]
#[must_use]
pub(crate) fn session_by_call(pid: u32) -> Option<u32> {
    if pid <= 1 {
        return None;
    }
    let pid = as_pid(pid)?;
    rustix::process::getsid(Some(pid))
        .ok()
        .map(|session| session.as_raw_nonzero().get().unsigned_abs())
}

fn refuse(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message.to_owned())
}

/// The numbers a signal must never be sent to, whoever asks and whatever is checked before: 0
/// and negative numbers mean whole groups and everybody; 1 is the init of the machine or the
/// unit; this process is the host itself.
fn checked_target(pid: u32) -> io::Result<rustix::process::Pid> {
    if pid <= 1 {
        return Err(refuse("process 1 and below are never signalled"));
    }
    if pid == own_pid() {
        return Err(refuse("the host does not signal itself"));
    }
    as_pid(pid).ok_or_else(|| refuse("that is not a process number"))
}

/// Sends `sig` to the process `pid`, which has to be the one that started at `expect_start_ms`
/// when that is given: a number that was reused since names another process, and nothing is sent.
/// Where the system has process handles the check and the signal are one step.
pub fn send_signal(pid: u32, expect_start_ms: Option<i64>, sig: Sig) -> io::Result<()> {
    let target = checked_target(pid)?;
    if let Some(done) = super::os::signal_via_handle(pid, expect_start_ms, sig) {
        return done;
    }
    if let Some(expected) = expect_start_ms {
        let found = super::os::exact_start_ms(pid);
        #[cfg(target_os = "macos")]
        let found = found.or_else(|| {
            use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
            let mut system = System::new();
            let id = Pid::from_u32(pid);
            system.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[id]),
                true,
                ProcessRefreshKind::nothing(),
            );
            system.process(id).map(|process| {
                i64::try_from(process.start_time())
                    .unwrap_or(0)
                    .saturating_mul(1000)
            })
        });
        if found != Some(expected) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "that number now names another process",
            ));
        }
    }
    rustix::process::kill_process(target, rustix_signal_of(sig)).map_err(io_error)
}

/// Sends `sig` to every member of the process group `group`, in one step: no member can be
/// missed by forking while the signal is sent. The group of the host itself, and groups 0 and 1,
/// are never signalled.
pub fn send_group(group: u32, sig: Sig) -> io::Result<()> {
    if group <= 1 {
        return Err(refuse("process groups 1 and below are never signalled"));
    }
    if own_process_group() == Some(group) {
        return Err(refuse("the host does not signal its own process group"));
    }
    let target = as_pid(group).ok_or_else(|| refuse("that is not a process group number"))?;
    rustix::process::kill_process_group(target, rustix_signal_of(sig)).map_err(io_error)
}
