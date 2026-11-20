//! Verified parts feed a joined native decoder; only the retained owned root is changed.

use super::*;
use crate::{
    blobs::{BlobSource, MemoryWriter},
    paths::{PathStyle, Workspace},
    platform::posix::{Dir, NamesCursor},
};
use orch_driver_contract::volume_stream::{Entry, HEADER_MAX, Kind, MAGIC};
use orch_link_contract::commands::{VOLUME_ARCHIVE_PART_MAX_BYTES, VolumeArchive};
use orch_types::Hash;
use std::{
    ffi::OsString,
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStringExt, fs::MetadataExt},
    },
    path::Path,
    time::Instant,
};
use tokio::sync::mpsc;

fn invalid(message: &str) -> HostError {
    HostError::Content(message.into())
}
fn io_error(action: &str, error: io::Error) -> HostError {
    if crate::error::cancellation().is_some_and(|cancel| cancel.is_cancelled()) {
        HostError::Cancelled
    } else {
        HostError::io(action, error)
    }
}
fn native_error(action: &str, error: rustix::io::Errno) -> HostError {
    io_error(action, io::Error::from_raw_os_error(error.raw_os_error()))
}
async fn cancelled() {
    match crate::error::cancellation() {
        Some(cancel) => cancel.cancelled().await,
        None => std::future::pending::<()>().await,
    }
}
enum FeedError {
    Source(HostError),
    ReaderClosed,
}

pub(super) async fn run(
    binding: DirectoryBinding,
    archive: VolumeArchive,
    source: &dyn BlobSource,
    progress: Arc<dyn Progress>,
) -> Result<()> {
    let size = archive.parts.iter().try_fold(0_i64, |sum, part| {
        (part.size > 0 && part.size <= VOLUME_ARCHIVE_PART_MAX_BYTES)
            .then_some(())
            .and_then(|()| sum.checked_add(part.size))
    });
    if archive.parts.is_empty() || size != Some(archive.size) {
        return Err(invalid(
            "the archive does not contain a complete ordered stream",
        ));
    }
    let (send, receive) = mpsc::channel::<Vec<u8>>(1);
    let feeding = async move {
        for part in &archive.parts {
            let work = async {
                let limit = usize::try_from(part.size)
                    .map_err(|_| invalid("the archive part size cannot be represented"))?;
                let mut output = MemoryWriter::with_limit(limit);
                let size = source
                    .get(part.hash, &mut output)
                    .await
                    .map_err(|error| HostError::Content(error.to_string()))?;
                let bytes = output.into_bytes();
                if size != part.size
                    || i64::try_from(bytes.len()).ok() != Some(part.size)
                    || Hash::of(&bytes) != part.hash
                {
                    return Err(invalid(
                        "the archive part does not have its announced hash and size",
                    ));
                }
                Ok::<_, HostError>(bytes)
            };
            let bytes = tokio::select! { () = cancelled() => return Err(FeedError::Source(HostError::Cancelled)), result = work => result.map_err(FeedError::Source)?, };
            tokio::select! { () = cancelled() => return Err(FeedError::Source(HostError::Cancelled)), result = send.send(bytes) => result.map_err(|_| FeedError::ReaderClosed)?, }
        }
        Ok::<_, FeedError>(())
    };
    let importing = crate::error::blocking(move || {
        let root = Workspace::open(Path::new(&binding.path), &[], PathStyle::Posix, true)?;
        let actual = root.root_meta()?;
        if binding.device.zip(binding.inode) != Some((actual.dev, actual.ino)) {
            return Err(HostError::Changed(
                "the original mount directory changed identity".into(),
            ));
        }
        decode(
            root.root_dir()?,
            &mut Parts {
                receive,
                current: io::Cursor::new(Vec::new()),
            },
            progress.as_ref(),
        )?;
        let current = std::fs::metadata(&binding.path)
            .map_err(|error| io_error("checking the completed mount destination", error))?;
        if binding.device.zip(binding.inode)
            != Some((
                current.dev(),
                crate::platform::FileNumber::from_u64(current.ino()),
            ))
        {
            return Err(HostError::Changed(
                "the mount destination changed identity during import".into(),
            ));
        }
        Ok(())
    });
    let (fed, imported) = tokio::join!(feeding, importing);
    if let Err(FeedError::Source(error)) = fed {
        return Err(error);
    }
    imported?;
    if matches!(fed, Err(FeedError::ReaderClosed)) {
        return Err(invalid("the native import stopped before the stream ended"));
    }
    Ok(())
}

pub(super) fn remove(binding: &DirectoryBinding, progress: &dyn Progress) -> Result<()> {
    let path = Path::new(&binding.path);
    let root = match Workspace::open(path, &[], PathStyle::Posix, true) {
        Ok(root) => root,
        Err(HostError::NotFound(_)) => return Ok(()),
        Err(error) => return Err(error),
    };
    let actual = root.root_meta()?;
    if binding.device.zip(binding.inode) != Some((actual.dev, actual.ino)) {
        return Err(HostError::Changed(
            "the owned mount directory changed identity".into(),
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| invalid("the owned directory has no parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| invalid("the owned directory has no name"))?;
    let parent = Dir::open_path(parent)
        .map_err(|error| io_error("holding the owned directory parent", error))?;
    clear(&root.root_dir()?, progress)?;
    let current = parent
        .lstat(name)
        .map_err(|error| io_error("checking the owned directory before removal", error))?;
    if !current.is_dir() || (current.dev, current.ino) != (actual.dev, actual.ino) {
        return Err(HostError::Changed(
            "the owned mount directory changed identity".into(),
        ));
    }
    parent
        .rmdir(name)
        .map_err(|error| io_error("removing the owned mount directory", error))?;
    parent
        .sync()
        .map_err(|error| io_error("flushing owned mount removal", error))
}

struct Parts {
    receive: mpsc::Receiver<Vec<u8>>,
    current: io::Cursor<Vec<u8>>,
}
impl Read for Parts {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        loop {
            crate::error::check_cancelled().map_err(io::Error::other)?;
            let count = self.current.read(output)?;
            if count != 0 {
                return Ok(count);
            }
            match self.receive.blocking_recv() {
                Some(bytes) => self.current = io::Cursor::new(bytes),
                None => return Ok(0),
            }
        }
    }
}

fn header(input: &mut impl Read) -> Result<Option<Entry>> {
    let mut length = [0; 4];
    input
        .read_exact(&mut length)
        .map_err(|error| io_error("reading mount metadata", error))?;
    let length = u32::from_le_bytes(length) as usize;
    if length == 0 {
        return Ok(None);
    }
    if length > HEADER_MAX {
        return Err(invalid(
            "the carried metadata exceeds the volume stream bound",
        ));
    }
    let mut bytes = vec![0; length];
    input
        .read_exact(&mut bytes)
        .map_err(|error| io_error("reading mount metadata", error))?;
    let entry: Entry =
        serde_json::from_slice(&bytes).map_err(|error| HostError::Content(error.to_string()))?;
    if entry.uid == u32::MAX
        || entry.gid == u32::MAX
        || entry.mode & !0o7777 != 0
        || !(0..1_000_000_000).contains(&entry.atime_ns)
        || !(0..1_000_000_000).contains(&entry.mtime_ns)
        || (!matches!(entry.kind, Kind::File) && entry.size != 0)
    {
        return Err(invalid("the carried entry has invalid native metadata"));
    }
    Ok(Some(entry))
}

fn components(path: &[u8]) -> Result<Vec<OsString>> {
    if path.is_empty() {
        return Ok(Vec::new());
    }
    path.split(|byte| *byte == b'/')
        .map(|part| {
            if part.is_empty() || part == b"." || part == b".." || part.contains(&0) {
                return Err(invalid(
                    "the carried path is not relative to its mount root",
                ));
            }
            Ok(OsString::from_vec(part.to_vec()))
        })
        .collect()
}
fn directory(root: &Dir, path: &[OsString]) -> Result<Dir> {
    let mut current = root
        .try_clone()
        .map_err(|error| io_error("holding the mount root", error))?;
    for name in path {
        current = current
            .open_child(name)
            .map_err(|error| io_error("opening a carried parent directory", error))?;
    }
    Ok(current)
}

struct Frame {
    directory: Dir,
    cursor: NamesCursor,
    name: Option<OsString>,
}
fn clear(root: &Dir, progress: &dyn Progress) -> Result<()> {
    let directory = root
        .try_clone()
        .map_err(|error| io_error("holding the mount root", error))?;
    let mut stack = vec![Frame {
        cursor: directory
            .names_cursor()
            .map_err(|error| io_error("reading the replaced mount", error))?,
        directory,
        name: None,
    }];
    let mut reported = Instant::now();
    while let Some(frame) = stack.last_mut() {
        crate::error::check_cancelled()?;
        let Some(name) = frame
            .cursor
            .next_name()
            .map_err(|error| io_error("reading the replaced mount", error))?
        else {
            let child = stack
                .pop()
                .ok_or_else(|| invalid("the mount cleanup lost its directory"))?;
            if let Some(name) = child.name {
                let parent = stack
                    .last()
                    .ok_or_else(|| invalid("the mount cleanup lost its parent"))?;
                let actual = parent
                    .directory
                    .lstat(&name)
                    .map_err(|error| io_error("checking a replaced directory", error))?;
                let original = child
                    .directory
                    .stat_self()
                    .map_err(|error| io_error("checking a held directory", error))?;
                if actual.identity() != original.identity() {
                    return Err(HostError::Changed(
                        "a replaced directory changed identity".into(),
                    ));
                }
                parent
                    .directory
                    .rmdir(&name)
                    .map_err(|error| io_error("removing a replaced directory", error))?;
            }
            continue;
        };
        let meta = frame
            .directory
            .lstat(&name)
            .map_err(|error| io_error("checking a replaced entry", error))?;
        if meta.is_dir() {
            let child = frame
                .directory
                .open_child(&name)
                .map_err(|error| io_error("opening a replaced directory", error))?;
            if child
                .stat_self()
                .map_err(|error| io_error("checking a replaced directory", error))?
                .identity()
                != meta.identity()
            {
                return Err(HostError::Changed(
                    "a replaced directory changed identity".into(),
                ));
            }
            stack.push(Frame {
                cursor: child
                    .names_cursor()
                    .map_err(|error| io_error("reading a replaced directory", error))?,
                directory: child,
                name: Some(name),
            });
        } else {
            frame
                .directory
                .unlink(&name)
                .map_err(|error| io_error("removing a replaced entry", error))?;
        }
        if reported.elapsed() >= crate::snapshot::PROGRESS_INTERVAL {
            progress.counting(0, 0, 0);
            reported = Instant::now();
        }
    }
    Ok(())
}

// Directory metadata is replayed backwards from an anonymous private file. This
// permits interleaved subtrees without holding every directory's attributes in RAM.
struct Directories {
    file: File,
}
impl Directories {
    fn new() -> Result<Self> {
        Ok(Self {
            file: tempfile::tempfile().map_err(|error| {
                io_error("creating the private directory metadata journal", error)
            })?,
        })
    }
    fn remember(&mut self, entry: &Entry) -> Result<()> {
        let bytes =
            serde_json::to_vec(entry).map_err(|error| HostError::Content(error.to_string()))?;
        let length = u32::try_from(bytes.len())
            .map_err(|_| invalid("the carried directory metadata cannot be represented"))?;
        self.file
            .write_all(&bytes)
            .and_then(|()| self.file.write_all(&length.to_le_bytes()))
            .map_err(|error| io_error("retaining carried directory metadata", error))
    }
    fn restore(&mut self, root: &Dir) -> Result<()> {
        let mut end = self
            .file
            .seek(SeekFrom::End(0))
            .map_err(|error| io_error("reading retained directory metadata", error))?;
        while end != 0 {
            crate::error::check_cancelled()?;
            if end < 4 {
                return Err(invalid("the private directory metadata is incomplete"));
            }
            self.file
                .seek(SeekFrom::Start(end - 4))
                .map_err(|error| io_error("reading retained directory metadata", error))?;
            let mut length = [0; 4];
            self.file
                .read_exact(&mut length)
                .map_err(|error| io_error("reading retained directory metadata", error))?;
            let length = u64::from(u32::from_le_bytes(length));
            let start = end
                .checked_sub(4 + length)
                .ok_or_else(|| invalid("the private directory metadata is incomplete"))?;
            self.file
                .seek(SeekFrom::Start(start))
                .map_err(|error| io_error("reading retained directory metadata", error))?;
            let mut bytes = vec![0; length as usize];
            self.file
                .read_exact(&mut bytes)
                .map_err(|error| io_error("reading retained directory metadata", error))?;
            let entry: Entry = serde_json::from_slice(&bytes)
                .map_err(|error| HostError::Content(error.to_string()))?;
            let target = directory(root, &components(&entry.path)?)?;
            metadata(
                &target
                    .metadata_handle()
                    .map_err(|error| io_error("pinning restored directory metadata", error))?,
                &entry,
            )?;
            target
                .sync()
                .map_err(|error| io_error("flushing a restored directory", error))?;
            end = start;
        }
        Ok(())
    }
}

fn metadata(handle: &File, entry: &Entry) -> Result<()> {
    use rustix::fs::{AtFlags, Mode, Timestamps, XattrFlags};
    use rustix::process::{Gid, Uid};
    let actual = handle
        .metadata()
        .map_err(|error| io_error("checking imported metadata", error))?;
    if (actual.uid(), actual.gid()) != (entry.uid, entry.gid) {
        rustix::fs::chownat(
            handle,
            "",
            Some(Uid::from_raw(entry.uid)),
            Some(Gid::from_raw(entry.gid)),
            AtFlags::EMPTY_PATH | AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|error| native_error("restoring a mount entry's owner", error))?;
    }
    let path = format!("/proc/self/fd/{}", handle.as_raw_fd());
    if !matches!(entry.kind, Kind::Link) {
        rustix::fs::chmodat(
            rustix::fs::CWD,
            &path,
            Mode::from_bits_truncate(entry.mode),
            AtFlags::empty(),
        )
        .map_err(|error| native_error("restoring a mount entry's mode", error))?;
    }
    for (name, value) in &entry.xattrs {
        crate::error::check_cancelled()?;
        rustix::fs::setxattr(&path, name.as_slice(), value, XattrFlags::empty())
            .map_err(|error| native_error("restoring a mount attribute", error))?;
    }
    let times = Timestamps {
        last_access: rustix::fs::Timespec {
            tv_sec: entry.atime,
            tv_nsec: entry.atime_ns,
        },
        last_modification: rustix::fs::Timespec {
            tv_sec: entry.mtime,
            tv_nsec: entry.mtime_ns,
        },
    };
    rustix::fs::utimensat(
        handle,
        "",
        &times,
        AtFlags::EMPTY_PATH | AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(|error| native_error("restoring mount entry times", error))
}

fn copy_file(
    input: &mut impl Read,
    file: &mut File,
    size: u64,
    progress: &dyn Progress,
    totals: &mut (i64, i64, i64),
    reported: &mut Instant,
) -> Result<()> {
    let mut left = size;
    let mut buffer = vec![0; crate::blobs::READ_CHUNK_BYTES];
    while left > 0 {
        crate::error::check_cancelled()?;
        let length = left.min(buffer.len() as u64) as usize;
        let count = input
            .read(&mut buffer[..length])
            .map_err(|error| io_error("reading carried file bytes", error))?;
        if count == 0 {
            return Err(invalid("the volume stream ended inside a file"));
        }
        file.write_all(&buffer[..count])
            .map_err(|error| io_error("restoring carried file bytes", error))?;
        left -= count as u64;
        totals.1 = totals
            .1
            .checked_add(count as i64)
            .ok_or_else(|| invalid("the restored byte count cannot be represented"))?;
        report(progress, *totals, reported, false);
    }
    file.sync_all()
        .map_err(|error| io_error("flushing a carried file", error))
}
fn report(
    progress: &dyn Progress,
    totals: (i64, i64, i64),
    last: &mut Instant,
    final_report: bool,
) {
    if final_report || last.elapsed() >= crate::snapshot::PROGRESS_INTERVAL {
        progress.counting(totals.0, totals.1, totals.2);
        *last = Instant::now();
    }
}

fn decode(root: Dir, input: &mut impl Read, progress: &dyn Progress) -> Result<()> {
    let mut magic = vec![0; MAGIC.len()];
    input
        .read_exact(&mut magic)
        .map_err(|error| io_error("reading the volume stream version", error))?;
    if magic != MAGIC {
        return Err(invalid("the volume stream version is unsupported"));
    }
    let first = header(input)?.ok_or_else(|| invalid("the volume stream has no root directory"))?;
    if !first.path.is_empty() || !matches!(first.kind, Kind::Directory) {
        return Err(invalid("the volume stream begins with its root directory"));
    }
    let mut directories = Directories::new()?;
    directories.remember(&first)?;
    progress.counting(0, 0, 1);
    clear(&root, progress)?;
    let mut totals = (0_i64, 0_i64, 1_i64);
    let mut reported = Instant::now();
    while let Some(entry) = header(input)? {
        crate::error::check_cancelled()?;
        let parts = components(&entry.path)?;
        let (name, parents) = parts
            .split_last()
            .ok_or_else(|| invalid("the volume stream names its root once"))?;
        let parent = directory(&root, parents)?;
        match entry.kind {
            Kind::Directory => {
                parent
                    .mkdir(name, 0o700)
                    .map_err(|error| io_error("creating a carried directory", error))?;
                directories.remember(&entry)?;
                totals.2 = totals
                    .2
                    .checked_add(1)
                    .ok_or_else(|| invalid("the restored directory count cannot be represented"))?;
            }
            Kind::File => {
                let mut file = parent
                    .create_new(name, 0o600)
                    .map_err(|error| io_error("creating a carried file", error))?;
                copy_file(
                    input,
                    &mut file,
                    entry.size,
                    progress,
                    &mut totals,
                    &mut reported,
                )?;
            }
            Kind::Link => {
                let target = entry
                    .link
                    .as_ref()
                    .filter(|target| !target.contains(&0))
                    .ok_or_else(|| invalid("the carried link has no valid target"))?;
                parent
                    .symlink(&OsString::from_vec(target.clone()), name)
                    .map_err(|error| io_error("creating a carried link", error))?;
            }
            Kind::HardLink => {
                let source = components(
                    entry
                        .link
                        .as_deref()
                        .ok_or_else(|| invalid("the carried hard link has no target"))?,
                )?;
                let (source_name, source_parents) = source
                    .split_last()
                    .ok_or_else(|| invalid("the carried hard link names a file"))?;
                let source_parent = directory(&root, source_parents)?;
                let handle = source_parent
                    .open_metadata(source_name)
                    .map_err(|error| io_error("pinning the carried hard link source", error))?;
                if !handle
                    .metadata()
                    .map_err(|error| io_error("checking the hard link source", error))?
                    .is_file()
                {
                    return Err(invalid(
                        "the carried hard link does not name an earlier regular file",
                    ));
                }
                parent
                    .link_file(&handle, name)
                    .map_err(|error| io_error("creating a carried hard link", error))?;
            }
            Kind::Character | Kind::Block | Kind::Fifo => {
                let kind = match entry.kind {
                    Kind::Character => rustix::fs::FileType::CharacterDevice,
                    Kind::Block => rustix::fs::FileType::BlockDevice,
                    _ => rustix::fs::FileType::Fifo,
                };
                let major = u32::try_from(entry.major)
                    .map_err(|_| invalid("the carried device number cannot be represented"))?;
                let minor = u32::try_from(entry.minor)
                    .map_err(|_| invalid("the carried device number cannot be represented"))?;
                parent
                    .make_special(name, kind, rustix::fs::makedev(major, minor))
                    .map_err(|error| io_error("creating a carried native entry", error))?;
            }
            Kind::Socket => parent
                .make_socket(name)
                .map_err(|error| io_error("creating a carried socket entry", error))?,
        }
        if !matches!(entry.kind, Kind::Directory) {
            let handle = parent
                .open_metadata(name)
                .map_err(|error| io_error("pinning carried metadata", error))?;
            metadata(&handle, &entry)?;
        }
        if matches!(entry.kind, Kind::File | Kind::HardLink) {
            totals.0 = totals
                .0
                .checked_add(1)
                .ok_or_else(|| invalid("the restored file count cannot be represented"))?;
        }
        report(progress, totals, &mut reported, false);
    }
    let mut extra = [0; 1];
    if input
        .read(&mut extra)
        .map_err(|error| io_error("checking the volume stream end", error))?
        != 0
    {
        return Err(invalid("the volume stream has trailing bytes"));
    }
    directories.restore(&root)?;
    report(progress, totals, &mut reported, true);
    Ok(())
}
