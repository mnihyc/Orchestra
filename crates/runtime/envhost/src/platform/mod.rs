//! Platform components.
//!
//! Everything that differs between operating systems lives under this module, so that the rest of
//! the package is written once. A [`Dir`] is a directory that was opened and is held by a handle:
//! every operation on the workspace is a name relative to such a handle, which is what keeps a
//! path from leaving the workspace root even when somebody swaps a directory for a link while a
//! request is being served. Linux and macOS share the POSIX implementation in [`posix`]; Windows
//! has the same functions on top of the standard library, and says in its own documentation
//! which of the guarantees it cannot give.
//!
//! The package contains no `unsafe` code: the system calls are made through `rustix`, whose
//! interface is safe.

pub mod linux;
pub mod macos;
#[cfg(unix)]
pub mod posix;
pub mod windows;

#[cfg(target_os = "linux")]
pub use linux as os;
#[cfg(target_os = "macos")]
pub use macos as os;
#[cfg(windows)]
pub use windows as os;

pub use os::{process_group_of, session_of};
#[cfg(unix)]
pub use posix::{
    Dir, file_meta, is_cross_device, own_pid, own_process_group, read_at, send_group, send_signal,
    set_file_mode, write_at,
};
#[cfg(windows)]
pub use windows::{
    Dir, file_meta, is_cross_device, own_pid, own_process_group, read_at, send_group, send_signal,
    set_file_mode, write_at,
};

/// A signal the host sends. `Stop` and `Continue` hold processes still and let them go; the three
/// others are what a person may send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sig {
    /// Interrupt, as the keyboard does.
    Int,
    /// Ask to end.
    Term,
    /// End at once.
    Kill,
    /// Hold still.
    Stop,
    /// Go on.
    Continue,
}

/// One row of the system's table of processes, the least that is needed to find a tree: it is
/// read for every process of the machine, so it is the cheap part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcRow {
    /// Its number.
    pub pid: u32,
    /// The number of the process that started it, where the system says.
    pub ppid: u32,
    /// Its process group, where the system has them; 0 where it is not told.
    pub group: u32,
    /// Its session, where the system has them; 0 where it is not told.
    pub session: u32,
    /// When it started, in milliseconds since 1970.
    pub start_ms: i64,
    /// What it is doing, as the system's letter for it (`R`, `S`, `D`, `T`, `Z`, ...).
    pub state: char,
}

/// Whether processes can be held still and let go again here.
#[cfg(unix)]
pub const CAN_SUSPEND: bool = true;

/// Whether processes can be held still and let go again here.
#[cfg(windows)]
pub const CAN_SUSPEND: bool = false;

/// Makes `command` start a process of a group of its own, so that it and what it starts can be
/// told apart from the rest of the machine and signalled together.
#[cfg(unix)]
pub fn prepare_group(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

/// Makes `command` start a process of a group of its own, so that it and what it starts can be
/// told apart from the rest of the machine and signalled together.
#[cfg(windows)]
pub fn prepare_group(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    /// The flag that gives a new process a console process group of its own.
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP);
}

/// What kind of thing a directory entry is, without following a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// A symbolic link.
    Symlink,
    /// A socket, a device, a pipe or anything else that is never read.
    Other,
}

/// What the system says of an entry, without following a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meta {
    /// What it is.
    pub kind: Kind,
    /// Its size in bytes.
    pub size: u64,
    /// When its content was last changed, in nanoseconds since 1970.
    pub mtime_ns: i64,
    /// When it was last changed in any way, in nanoseconds since 1970; zero where the system does
    /// not say. A program cannot set this one, which is why a cached hash is keyed by it.
    pub ctime_ns: i64,
    /// Its permission bits.
    pub mode: u32,
    /// Its number on its device; zero where the system has none.
    pub ino: u64,
    /// The device it lives on; zero where the system has none.
    pub dev: u64,
    /// How many names it has.
    pub nlink: u64,
}

impl Meta {
    /// Whether it is a regular file.
    #[must_use]
    pub fn is_file(&self) -> bool {
        self.kind == Kind::File
    }

    /// Whether it is a directory.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir
    }

    /// Whether it is a symbolic link.
    #[must_use]
    pub fn is_symlink(&self) -> bool {
        self.kind == Kind::Symlink
    }

    /// The part of this that says whether a file is still the version somebody saw: it differs
    /// whenever the content, the size or the file itself was changed, even by a program that sets
    /// the modification time back.
    #[must_use]
    pub fn identity(&self) -> Identity {
        Identity {
            dev: self.dev,
            ino: self.ino,
            size: self.size,
            mtime_ns: self.mtime_ns,
            ctime_ns: self.ctime_ns,
        }
    }
}

/// Which version of which file a [`Meta`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Identity {
    dev: u64,
    ino: u64,
    size: u64,
    mtime_ns: i64,
    ctime_ns: i64,
}

/// Whether file names differ by case where the system has no word on it: the default of the
/// platform, used when a directory cannot be probed.
pub const DEFAULT_CASE_SENSITIVE: bool = os::CASE_SENSITIVE_BY_DEFAULT;

/// Seconds-and-nanoseconds as nanoseconds since 1970, saturating at the ends of the range.
#[must_use]
pub fn unix_nanos(seconds: i64, nanos: i64) -> i64 {
    seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
compile_error!("the environment host supports Linux, macOS and Windows");
