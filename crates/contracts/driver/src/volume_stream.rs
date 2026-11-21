//! The existing whole-volume stream metadata, shared by native storage producers.
//! Each JSON header follows a little-endian u32 byte length; a regular file's raw bytes
//! follow its header, and a zero header length closes the stream.

use serde::{Deserialize, Serialize};

/// The version already used by the shipped static volume helper.
pub const MAGIC: &[u8] = b"ORCH-VOLUME-1\n";
/// The existing wire bound for a single metadata header, independent from file size.
pub const HEADER_MAX: usize = 4 * 1024 * 1024;

/// Native kinds retain links and Unix special files rather than silently dropping them.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A directory, including the root header.
    Directory,
    /// A regular file whose bytes follow its header.
    File,
    /// An opaque symbolic link.
    Link,
    /// Another name for an earlier regular file.
    HardLink,
    /// A native character device.
    Character,
    /// A native block device.
    Block,
    /// A named pipe.
    Fifo,
    /// A filesystem socket entry.
    Socket,
}

/// One preserved native entry. Paths and link targets retain their original bytes.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Relative path, empty only for the stream's first root directory.
    pub path: Vec<u8>,
    /// Native entry kind.
    pub kind: Kind,
    /// Raw file bytes following this header, zero for the other kinds.
    pub size: u64,
    /// Original Unix owner and group.
    pub uid: u32,
    /// Original Unix group.
    pub gid: u32,
    /// Original permission and special mode bits.
    pub mode: u32,
    /// Original access and modification times, split into seconds and nanoseconds.
    pub atime: i64,
    /// Access-time nanoseconds.
    pub atime_ns: i64,
    /// Modification-time seconds.
    pub mtime: i64,
    /// Modification-time nanoseconds.
    pub mtime_ns: i64,
    /// Link target, or an earlier relative regular-file path for a hard link.
    pub link: Option<Vec<u8>>,
    /// Original device numbers for native device entries.
    pub major: u64,
    /// Original minor device number.
    pub minor: u64,
    /// Original extended attribute names and raw values.
    pub xattrs: Vec<(Vec<u8>, Vec<u8>)>,
}
