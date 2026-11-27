//! Windows paths, job objects and ConPTY.
//!
//! The same functions as the POSIX implementation, on top of the standard library and the
//! process table of `sysinfo`, with relative opens isolated in one private native boundary.
//! Remaining platform limits are stated alongside their operations:
//!
//! - A [`Dir`] holds its original directory handle. Child opens and creation use that handle.
//!   Complete native identity and directory enforcement still await proof before runtime verification.
//! - A file has no permission bits beyond "read-only": a mode is kept as 0o444 or 0o666 (and
//!   0o111 more for a directory), and the bits of a mode that was asked for are not stored.
//! - A process cannot be asked to end, interrupted, held still or let go by number: `Term` ends it
//!   at once as `Kill` does, and `Int`, `Stop` and `Continue` are refused, which is why the
//!   environment says that processes cannot be held still here.
//! - There are no process groups and no sessions: a tree is found by parentage alone, and a
//!   group cannot be signalled. Since processes cannot be suspended, a child created between
//!   the last tree scan and its parent ending can escape that scan.
//!
//! Native process controls have target-runner proof. Directory containment remains unverified.

#![cfg(windows)]

#[allow(
    unsafe_code,
    reason = "only this private Windows boundary owns NT pointers and converts successful handles to File"
)]
mod native;

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

fn meta_of(file: &File) -> io::Result<Meta> {
    let metadata = file.metadata()?;
    let native = native::file_facts(file)?;
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
    Ok(Meta {
        kind,
        size: metadata.len(),
        mtime_ns: nanos_of(metadata.modified()),
        ctime_ns: native.changed,
        mode: if kind == Kind::Dir {
            base | MODE_SEARCH
        } else {
            base
        },
        ino: native.number,
        dev: native.volume,
        nlink: native.links,
    })
}

/// A directory held by its original native handle; its path is only for display.
#[derive(Debug)]
pub struct Dir {
    path: PathBuf,
    file: File,
}

impl Dir {
    /// Opens the directory at `path`, following links in the path: the root of a workspace, which
    /// is given by whoever set the environment up and is not a request's to name.
    pub fn open_path(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .share_mode(7)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?;
        if !file.metadata()?.is_dir() {
            return Err(io::Error::from(io::ErrorKind::NotADirectory));
        }
        Ok(Self {
            path: path.to_path_buf(),
            file,
        })
    }

    /// Opens the directory called `name` in this one. A link called `name` is not followed.
    pub fn open_child(&self, name: &OsStr) -> io::Result<Self> {
        plain(name)?;
        let file = native::open_directory(&self.file, name)?;
        let metadata = file.metadata()?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(io::Error::from(io::ErrorKind::NotADirectory));
        }
        Ok(Self {
            path: self.path.join(name),
            file,
        })
    }

    /// Another handle on the same directory.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            path: self.path.clone(),
            file: self.file.try_clone()?,
        })
    }

    /// Where this directory was opened.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What this directory itself is.
    pub fn stat_self(&self) -> io::Result<Meta> {
        meta_of(&self.file)
    }

    /// Ask for the rights to traverse and add files and subdirectories on this held directory.
    pub(crate) fn require_write(&self) -> io::Result<()> {
        native::require_directory_write(&self.file)
    }

    /// What the entry called `name` is, without following it.
    pub fn lstat(&self, name: &OsStr) -> io::Result<Meta> {
        plain(name)?;
        meta_of(&native::open_metadata(&self.file, name)?)
    }

    /// Where the link called `name` points, as written.
    pub fn read_link(&self, name: &OsStr) -> io::Result<OsString> {
        plain(name)?;
        native::read_link(&native::open_metadata(&self.file, name)?)
    }

    /// Tests the native account's access without reading file content or opening special files.
    pub fn check_readable(&self, name: &OsStr, kind: Kind) -> io::Result<()> {
        match kind {
            Kind::File => self.open_read(name).map(drop),
            Kind::Dir => {
                let directory = self.open_child(name)?;
                std::fs::read_dir(directory.path()).map(drop)
            }
            Kind::Symlink | Kind::Other => Ok(()),
        }
    }

    /// Opens the regular file called `name` for reading. A link is opened as the link it is, which
    /// is not a regular file, so it is refused.
    pub fn open_read(&self, name: &OsStr) -> io::Result<File> {
        plain(name)?;
        let file = native::open_read(&self.file, name)?;
        let metadata = file.metadata()?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        Ok(file)
    }

    /// Opens an existing regular temporary file for resuming, relative to this original handle and
    /// without following a link.
    pub fn open_update(&self, name: &OsStr) -> io::Result<File> {
        plain(name)?;
        let file = native::open_update(&self.file, name)?;
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
        native::create_file(&self.file, name)
    }

    /// Makes the directory called `name` relative to this original handle.
    pub fn mkdir(&self, name: &OsStr, _mode: u32) -> io::Result<()> {
        plain(name)?;
        drop(native::create_directory(&self.file, name)?);
        Ok(())
    }

    /// Makes a link called `name` that points to `target`. Making one takes a privilege that an
    /// ordinary account may lack, in which case the system's refusal comes back.
    pub fn symlink(&self, target: &OsStr, name: &OsStr) -> io::Result<()> {
        plain(name)?;
        native::create_link(&self.file, target, name)
    }

    /// Removes the file or link called `name`.
    pub fn unlink(&self, name: &OsStr) -> io::Result<()> {
        plain(name)?;
        let file = native::open_delete(&self.file, name)?;
        let metadata = file.metadata()?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            return Err(io::Error::from(io::ErrorKind::IsADirectory));
        }
        native::delete(&file)
    }

    /// Removes the empty directory itself through its opened handle.
    pub fn rmdir(&self, name: &OsStr) -> io::Result<()> {
        plain(name)?;
        let file = native::open_delete(&self.file, name)?;
        let metadata = file.metadata()?;
        if !metadata.is_dir() && !metadata.file_type().is_symlink_dir() {
            return Err(io::Error::from(io::ErrorKind::NotADirectory));
        }
        native::delete(&file)
    }

    /// Renames the opened source into the destination directory with one native replace decision.
    pub fn rename_to(
        &self,
        from: &OsStr,
        to_dir: &Dir,
        to: &OsStr,
        replace: bool,
    ) -> io::Result<()> {
        plain(from)?;
        plain(to)?;
        let file = native::open_delete(&self.file, from)?;
        native::rename(&file, &to_dir.file, to, replace)
    }

    /// The names in this directory, without `.` and `..`, in no particular order.
    pub fn names(&self) -> io::Result<Vec<OsString>> {
        native::names(&self.file)?.collect()
    }

    /// Streams the names of the original held directory, one native page at a time.
    pub fn names_stream(&self) -> io::Result<Box<dyn Iterator<Item = io::Result<OsString>>>> {
        Ok(Box::new(native::names(&self.file)?))
    }

    /// Visits names until the caller stops or the native directory cursor ends.
    pub fn visit_names(&self, visit: &mut dyn FnMut(&OsStr) -> bool) -> io::Result<bool> {
        for name in native::names(&self.file)? {
            if !visit(&name?) {
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
        let file = native::open_attributes(&self.file, name)?;
        let metadata = file.metadata()?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a link has no mode of its own",
            ));
        }
        let mut permissions = metadata.permissions();
        permissions.set_readonly(mode & 0o222 == 0);
        file.set_permissions(permissions)
    }

    /// Sets the modification time on the opened object, never by following a mutable leaf name.
    pub fn set_mtime(&self, name: &OsStr, mtime_ns: i64) -> io::Result<()> {
        plain(name)?;
        let file = native::open_attributes(&self.file, name)?;
        file.set_modified(nanos_to_time(mtime_ns))
    }
}

/// What an open file is, as it is at this moment.
pub fn file_meta(file: &File) -> io::Result<Meta> {
    meta_of(file)
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

#[cfg(test)]
mod directory_identity_cases {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn a_held_directory_never_reads_a_replacement_ancestors_data() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("root");
        let moved = scratch.path().join("original");
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("value"), b"original").unwrap();
        // Establish that the test account can move this directory before it is held.
        std::fs::rename(&root, &moved).unwrap();
        std::fs::rename(&moved, &root).unwrap();
        let directory = Dir::open_path(&nested).unwrap();
        if std::fs::rename(&root, &moved).is_ok() {
            std::fs::create_dir_all(&nested).unwrap();
            std::fs::write(nested.join("value"), b"replacement").unwrap();
        }
        std::fs::write(directory.path().join("current-label"), b"current").unwrap();
        let physical_root = if moved.is_dir() { &moved } else { &root };
        std::fs::write(physical_root.join("nested/original-label"), b"original").unwrap();
        let names = directory
            .names_stream()
            .unwrap()
            .collect::<io::Result<Vec<_>>>()
            .unwrap();
        assert!(names.contains(&OsString::from("original-label")));
        if moved.is_dir() {
            assert!(!names.contains(&OsString::from("current-label")));
        }
        let mut bytes = Vec::new();
        directory
            .open_read(OsStr::new("value"))
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, b"original");
    }

    #[test]
    fn creating_a_child_is_exclusive_and_directory_children_keep_the_original_parent() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let directory = Dir::open_path(&root).unwrap();
        directory
            .create_new(OsStr::new("kept"), MODE_WRITABLE)
            .unwrap()
            .write_all(b"original")
            .unwrap();
        assert_eq!(
            directory
                .create_new(OsStr::new("kept"), MODE_WRITABLE)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(root.join("kept")).unwrap(), b"original");
        directory
            .mkdir(OsStr::new("nested"), MODE_WRITABLE)
            .unwrap();
        let nested = directory.open_child(OsStr::new("nested")).unwrap();
        let moved = scratch.path().join("moved");
        let relocated = std::fs::rename(&root, &moved).is_ok();
        if relocated {
            std::fs::create_dir_all(root.join("nested")).unwrap();
        }
        nested
            .create_new(OsStr::new("child"), MODE_WRITABLE)
            .unwrap()
            .write_all(b"owned")
            .unwrap();
        let actual = if relocated { &moved } else { &root };
        assert_eq!(
            std::fs::read(actual.join("nested/child")).unwrap(),
            b"owned"
        );
        if relocated {
            assert!(!root.join("nested/child").exists());
        }
    }

    #[test]
    fn resuming_a_file_opens_the_original_directorys_file_and_never_creates_one() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let directory = Dir::open_path(&root).unwrap();
        assert_eq!(
            directory
                .open_update(OsStr::new("partial"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        assert!(!root.join("partial").exists());
        directory
            .create_new(OsStr::new("partial"), MODE_WRITABLE)
            .unwrap()
            .write_all(b"first ")
            .unwrap();
        let moved = scratch.path().join("moved");
        let relocated = std::fs::rename(&root, &moved).is_ok();
        if relocated {
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join("partial"), b"replacement").unwrap();
        }
        let mut resumed = directory.open_update(OsStr::new("partial")).unwrap();
        let mut bytes = Vec::new();
        resumed.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"first ");
        resumed.write_all(b"second").unwrap();
        drop(resumed);
        let actual = if relocated { &moved } else { &root };
        assert_eq!(
            std::fs::read(actual.join("partial")).unwrap(),
            b"first second"
        );
        if relocated {
            assert_eq!(std::fs::read(root.join("partial")).unwrap(), b"replacement");
        }
        directory
            .mkdir(OsStr::new("folder"), MODE_WRITABLE)
            .unwrap();
        assert!(directory.open_update(OsStr::new("folder")).is_err());
    }

    #[test]
    fn a_held_directory_keeps_leaf_mutations_on_the_original_objects_after_ancestor_replacement() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("root");
        let nested = root.join("nested");
        std::fs::create_dir_all(nested.join("empty")).unwrap();
        std::fs::write(nested.join("data"), b"original").unwrap();
        let directory = Dir::open_path(&nested).unwrap();
        let moved = scratch.path().join("original");
        let relocated = std::fs::rename(&root, &moved).is_ok();
        if relocated {
            std::fs::create_dir_all(nested.join("empty")).unwrap();
            std::fs::write(nested.join("data"), b"replacement").unwrap();
            std::fs::write(nested.join("empty/keep"), b"other directory").unwrap();
        }
        let actual = if relocated {
            moved.join("nested")
        } else {
            nested.clone()
        };
        directory
            .set_mode(OsStr::new("data"), MODE_READ_ONLY)
            .unwrap();
        assert!(
            std::fs::metadata(actual.join("data"))
                .unwrap()
                .permissions()
                .readonly()
        );
        if relocated {
            assert!(
                !std::fs::metadata(nested.join("data"))
                    .unwrap()
                    .permissions()
                    .readonly()
            );
        }
        directory
            .set_mode(OsStr::new("data"), MODE_WRITABLE)
            .unwrap();
        directory
            .set_mtime(OsStr::new("data"), 1_700_000_000_000_000_000)
            .unwrap();
        assert_eq!(
            nanos_of(std::fs::metadata(actual.join("data")).unwrap().modified()),
            1_700_000_000_000_000_000
        );
        directory
            .rename_to(OsStr::new("data"), &directory, OsStr::new("renamed"), false)
            .unwrap();
        assert_eq!(std::fs::read(actual.join("renamed")).unwrap(), b"original");
        directory.unlink(OsStr::new("renamed")).unwrap();
        directory.rmdir(OsStr::new("empty")).unwrap();
        assert!(!actual.join("renamed").exists());
        assert!(!actual.join("empty").exists());
        if relocated {
            assert_eq!(std::fs::read(nested.join("data")).unwrap(), b"replacement");
            assert_eq!(
                std::fs::read(nested.join("empty/keep")).unwrap(),
                b"other directory"
            );
        }
    }

    #[test]
    fn a_non_replacing_native_rename_keeps_an_existing_destination_and_both_original_bytes() {
        let scratch = tempfile::tempdir().unwrap();
        let directory = Dir::open_path(scratch.path()).unwrap();
        std::fs::write(scratch.path().join("source"), b"source").unwrap();
        std::fs::write(scratch.path().join("target"), b"target").unwrap();
        assert_eq!(
            directory
                .rename_to(
                    OsStr::new("source"),
                    &directory,
                    OsStr::new("target"),
                    false
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            std::fs::read(scratch.path().join("source")).unwrap(),
            b"source"
        );
        assert_eq!(
            std::fs::read(scratch.path().join("target")).unwrap(),
            b"target"
        );
        directory
            .rename_to(OsStr::new("source"), &directory, OsStr::new("target"), true)
            .unwrap();
        assert_eq!(
            std::fs::read(scratch.path().join("target")).unwrap(),
            b"source"
        );
    }

    #[test]
    fn held_directory_enumerations_are_original_independent_and_can_stop_without_collecting_the_rest()
     {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("root");
        std::fs::create_dir(&root).unwrap();
        for name in ["first", "second", "third"] {
            std::fs::write(root.join(name), name).unwrap();
        }
        let directory = Dir::open_path(&root).unwrap();
        let clone = directory.try_clone().unwrap();
        let moved = scratch.path().join("original");
        if std::fs::rename(&root, &moved).is_ok() {
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join("foreign"), b"not this directory").unwrap();
        }
        let expected: std::collections::BTreeSet<_> = ["first", "second", "third"]
            .into_iter()
            .map(OsString::from)
            .collect();
        assert_eq!(
            directory
                .names()
                .unwrap()
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            expected
        );
        let mut outer = std::collections::BTreeSet::new();
        directory
            .visit_names(&mut |name| {
                outer.insert(name.to_os_string());
                assert_eq!(
                    clone
                        .names()
                        .unwrap()
                        .into_iter()
                        .collect::<std::collections::BTreeSet<_>>(),
                    expected
                );
                true
            })
            .unwrap();
        assert_eq!(outer, expected);
        let mut count = 0;
        assert!(
            !directory
                .visit_names(&mut |_| {
                    count += 1;
                    false
                })
                .unwrap()
        );
        assert_eq!(count, 1);
    }

    #[test]
    fn changing_a_held_directory_into_a_junction_never_redirects_child_creation() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("root");
        let outside = scratch.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let directory = Dir::open_path(&root).unwrap();
        native::test_set_junction(&directory.file, &std::fs::canonicalize(&outside).unwrap())
            .unwrap();
        let created = directory.create_new(OsStr::new("must-not-escape"), MODE_WRITABLE);
        assert!(!outside.join("must-not-escape").exists());
        if let Ok(file) = created {
            drop(file);
        }
        assert!(directory.read_link(OsStr::new("anything")).is_err());
    }

    #[test]
    fn reading_a_junction_uses_its_original_opened_parent_after_an_ancestor_moves() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("root");
        let outside = scratch.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let directory = Dir::open_path(&root).unwrap();
        directory.mkdir(OsStr::new("link"), MODE_WRITABLE).unwrap();
        let handle = native::open_attributes(&directory.file, OsStr::new("link")).unwrap();
        let target = std::fs::canonicalize(&outside).unwrap();
        native::test_set_junction(&handle, &target).unwrap();
        drop(handle);
        let original = directory.read_link(OsStr::new("link")).unwrap();
        assert!(Path::new(&original).is_absolute());
        let moved = scratch.path().join("original");
        if std::fs::rename(&root, &moved).is_ok() {
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join("link"), b"foreign").unwrap();
        }
        assert_eq!(directory.read_link(OsStr::new("link")).unwrap(), original);
    }

    #[test]
    fn creating_relative_file_and_directory_links_uses_the_held_parent_after_an_ancestor_moves() {
        let scratch = tempfile::tempdir().unwrap();
        std::os::windows::fs::symlink_file("missing", scratch.path().join("baseline"))
            .expect("the native account can create symbolic links");
        let root = scratch.path().join("root");
        std::fs::create_dir_all(root.join("folder")).unwrap();
        std::fs::write(root.join("target"), b"original").unwrap();
        let directory = Dir::open_path(&root).unwrap();
        let moved = scratch.path().join("original");
        let relocated = std::fs::rename(&root, &moved).is_ok();
        if relocated {
            std::fs::create_dir(&root).unwrap();
        }
        directory
            .symlink(OsStr::new("target"), OsStr::new("file-link"))
            .unwrap();
        directory
            .symlink(OsStr::new("folder"), OsStr::new("directory-link"))
            .unwrap();
        assert_eq!(
            directory.read_link(OsStr::new("file-link")).unwrap(),
            "target"
        );
        assert_eq!(
            directory.read_link(OsStr::new("directory-link")).unwrap(),
            "folder"
        );
        let actual = if relocated { &moved } else { &root };
        assert_eq!(
            std::fs::read(actual.join("file-link")).unwrap(),
            b"original"
        );
        assert!(
            std::fs::metadata(actual.join("directory-link"))
                .unwrap()
                .is_dir()
        );
        if relocated {
            assert!(!root.join("file-link").exists());
            assert!(!root.join("directory-link").exists());
        }
    }

    #[test]
    fn a_new_symbolic_link_never_changes_an_existing_leaf_or_leaves_an_invalid_target_placeholder()
    {
        let scratch = tempfile::tempdir().unwrap();
        std::fs::write(scratch.path().join("kept"), b"original").unwrap();
        let directory = Dir::open_path(scratch.path()).unwrap();
        assert_eq!(
            directory
                .symlink(OsStr::new("missing"), OsStr::new("kept"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            std::fs::read(scratch.path().join("kept")).unwrap(),
            b"original"
        );
        assert!(
            directory
                .symlink(OsStr::new("bad\0target"), OsStr::new("invalid"))
                .is_err()
        );
        assert!(directory.lstat(OsStr::new("invalid")).is_err());
    }

    #[test]
    fn handle_relative_links_preserve_the_native_unprivileged_creation_capability_and_restore_the_thread()
     {
        let scratch = tempfile::tempdir().unwrap();
        let directory = Dir::open_path(scratch.path()).unwrap();
        native::test_without_symlink_privilege(|| {
            let ordinary =
                std::os::windows::fs::symlink_file("missing", scratch.path().join("ordinary"));
            let confined = directory.symlink(OsStr::new("missing"), OsStr::new("confined"));
            assert_eq!(
                confined.is_ok(),
                ordinary.is_ok(),
                "ordinary={ordinary:?}, confined={confined:?}"
            );
            if confined.is_ok() {
                assert_eq!(
                    directory.read_link(OsStr::new("confined")).unwrap(),
                    "missing"
                );
            } else {
                assert!(directory.lstat(OsStr::new("confined")).is_err());
            }
        })
        .unwrap();
        std::os::windows::fs::symlink_file("missing", scratch.path().join("afterwards"))
            .expect("the restricted token did not escape its synchronous scope");
    }

    #[test]
    fn distinct_windows_files_with_equal_bytes_and_times_keep_distinct_native_numbers_and_real_link_counts()
     {
        let scratch = tempfile::tempdir().unwrap();
        let first = scratch.path().join("first");
        let second = scratch.path().join("second");
        std::fs::write(&first, b"same").unwrap();
        std::fs::write(&second, b"same").unwrap();
        std::fs::hard_link(&first, scratch.path().join("alias")).unwrap();
        let directory = Dir::open_path(scratch.path()).unwrap();
        for name in ["first", "second"] {
            directory
                .set_mtime(OsStr::new(name), 1_700_000_000_000_000_000)
                .unwrap();
        }
        let a = directory.lstat(OsStr::new("first")).unwrap();
        let b = directory.lstat(OsStr::new("second")).unwrap();
        let alias = directory.lstat(OsStr::new("alias")).unwrap();
        assert_eq!((a.size, a.mtime_ns), (b.size, b.mtime_ns));
        assert_ne!((a.dev, a.ino), (b.dev, b.ino));
        assert_eq!((a.dev, a.ino), (alias.dev, alias.ino));
        assert_eq!(a.nlink, 2);
        assert_eq!(b.nlink, 1);
        let opened = directory.open_read(OsStr::new("first")).unwrap();
        directory
            .rename_to(OsStr::new("first"), &directory, OsStr::new("moved"), false)
            .unwrap();
        let after = file_meta(&opened).unwrap();
        assert_eq!((after.dev, after.ino), (a.dev, a.ino));
    }

    #[test]
    fn windows_metadata_changes_advance_the_change_time_while_creation_time_stays_fixed() {
        let scratch = tempfile::tempdir().unwrap();
        let path = scratch.path().join("file");
        std::fs::write(&path, b"same").unwrap();
        let directory = Dir::open_path(scratch.path()).unwrap();
        let created = std::fs::metadata(&path).unwrap().created().unwrap();
        let before = directory.lstat(OsStr::new("file")).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        directory
            .set_mode(OsStr::new("file"), MODE_READ_ONLY)
            .unwrap();
        let after = directory.lstat(OsStr::new("file")).unwrap();
        directory
            .set_mode(OsStr::new("file"), MODE_WRITABLE)
            .unwrap();
        assert_eq!(std::fs::metadata(path).unwrap().created().unwrap(), created);
        assert_ne!(after.ctime_ns, before.ctime_ns);
        assert_eq!((after.dev, after.ino), (before.dev, before.ino));
    }

    #[test]
    fn selecting_a_windows_workspace_uses_its_held_allowed_root_after_an_ancestor_moves() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("root");
        std::fs::create_dir_all(root.join("project")).unwrap();
        std::fs::write(root.join("project/file"), b"original").unwrap();
        let held = Dir::open_path(&root).unwrap();
        if std::fs::rename(&root, scratch.path().join("original")).is_ok() {
            std::fs::create_dir_all(root.join("project")).unwrap();
            std::fs::write(root.join("project/file"), b"replacement").unwrap();
        }
        let workspace = crate::paths::Workspace::open_beneath(
            held,
            Path::new("project"),
            &[],
            crate::paths::PathStyle::Windows,
            false,
        )
        .unwrap();
        let file = workspace
            .locate(
                &workspace.parse("file").unwrap(),
                crate::paths::Locate::THROUGH,
            )
            .unwrap();
        let mut bytes = Vec::new();
        file.parent
            .open_read(file.name().unwrap())
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, b"original");
    }

    #[test]
    fn a_windows_allowed_root_changed_into_a_junction_is_refused_before_workspace_selection() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("root");
        let outside = scratch.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let resolved = std::fs::canonicalize(&root).unwrap();
        let directory = Dir::open_path(&root).unwrap();
        native::test_set_junction(&directory.file, &std::fs::canonicalize(outside).unwrap())
            .unwrap();
        assert!(crate::paths::Workspace::open_root_without_links(&resolved).is_err());
    }

    #[test]
    fn a_cloned_directory_keeps_its_original_identity_and_can_still_create_children() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("root");
        let moved = scratch.path().join("original");
        std::fs::create_dir(&root).unwrap();
        let directory = Dir::open_path(&root).unwrap();
        let clone = directory.try_clone().unwrap();
        drop(directory);
        let relocated = std::fs::rename(&root, &moved).is_ok();
        if relocated {
            std::fs::create_dir(&root).unwrap();
        }
        clone
            .create_new(OsStr::new("child"), MODE_WRITABLE)
            .unwrap()
            .write_all(b"kept")
            .unwrap();
        let actual = if relocated { &moved } else { &root };
        assert_eq!(std::fs::read(actual.join("child")).unwrap(), b"kept");
        if relocated {
            assert!(!root.join("child").exists());
        }
        drop(clone);
    }
}
