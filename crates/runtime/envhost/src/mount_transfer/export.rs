//! A native encoder and one bounded upload queue share the command's cancellation owner.

use super::*;
use crate::{
    blobs::{BlobSink, MemoryReader},
    paths::{PathStyle, Workspace},
    platform::posix::{Dir, NamesCursor},
};
use orch_driver_contract::volume_stream::{Entry, HEADER_MAX, Kind, MAGIC};
use orch_link_contract::commands::{BlobRef, VOLUME_ARCHIVE_PART_MAX_BYTES, VolumeArchive};
use orch_types::Hash;
use std::{
    collections::BTreeMap,
    fs::{File, Metadata},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{FileTypeExt, MetadataExt},
        },
    },
    path::Path,
    time::Instant,
};
use tokio::sync::mpsc;

fn io_error(action: &str, error: io::Error) -> HostError {
    if crate::error::cancellation().is_some_and(|cancel| cancel.is_cancelled()) {
        HostError::Cancelled
    } else {
        HostError::io(action, error)
    }
}

async fn cancelled() {
    match crate::error::cancellation() {
        Some(cancel) => cancel.cancelled().await,
        None => std::future::pending::<()>().await,
    }
}

pub(super) async fn run(
    binding: DirectoryBinding,
    part_bytes: i64,
    sink: &dyn BlobSink,
    progress: Arc<dyn Progress>,
) -> Result<VolumeArchive> {
    if !(1..=VOLUME_ARCHIVE_PART_MAX_BYTES).contains(&part_bytes) {
        return Err(HostError::Unsupported(
            "the volume part size is outside the content protocol".into(),
        ));
    }
    let limit = usize::try_from(part_bytes)
        .map_err(|_| HostError::Unsupported("the volume part size cannot be represented".into()))?;
    // One queued part overlaps the native reader with the current content upload.
    // Neither side collects an archive or a directory-sized listing.
    let (send, mut receive) = mpsc::channel::<Vec<u8>>(1);
    let encode = crate::error::blocking(move || {
        let root = Workspace::open(Path::new(&binding.path), &[], PathStyle::Posix, true)?;
        let actual = root.root_meta()?;
        if binding.device.zip(binding.inode) != Some((actual.dev, actual.ino)) {
            return Err(HostError::Changed(
                "the original mount directory changed identity".into(),
            ));
        }
        let directory = root.root_dir()?;
        let mut writer = Parts {
            send,
            bytes: Vec::new(),
            limit,
        };
        encode_tree(directory, &mut writer, progress.as_ref())
    });
    let upload = async move {
        let mut archive = VolumeArchive {
            parts: Vec::new(),
            size: 0,
        };
        loop {
            let bytes = tokio::select! {
                () = cancelled() => return Err(HostError::Cancelled),
                bytes = receive.recv() => match bytes { Some(bytes) => bytes, None => break },
            };
            let hash = Hash::of(&bytes);
            let size = i64::try_from(bytes.len()).map_err(|_| {
                HostError::Unsupported("the volume part size cannot be represented".into())
            })?;
            let put = async {
                if !sink
                    .missing(&[hash])
                    .await
                    .map_err(|error| HostError::Content(error.to_string()))?
                    .is_empty()
                {
                    sink.put(hash, size, &mut MemoryReader::new(bytes))
                        .await
                        .map_err(|error| HostError::Content(error.to_string()))?;
                }
                Ok::<_, HostError>(())
            };
            tokio::select! {
                () = cancelled() => return Err(HostError::Cancelled),
                result = put => result?,
            }
            archive.size = archive.size.checked_add(size).ok_or_else(|| {
                HostError::Unsupported("the volume stream size cannot be represented".into())
            })?;
            archive.parts.push(BlobRef { hash, size });
        }
        Ok(archive)
    };
    let (encoded, uploaded) = tokio::join!(encode, upload);
    // Dropping the receiver unblocks a producer after a failed upload. Both owners
    // finish before the command can acknowledge its success, failure or cancellation.
    let archive = uploaded?;
    encoded?;
    Ok(archive)
}

struct Parts {
    send: mpsc::Sender<Vec<u8>>,
    bytes: Vec<u8>,
    limit: usize,
}
impl Parts {
    fn send_part(&mut self) -> io::Result<()> {
        if self.bytes.is_empty() {
            return Ok(());
        }
        self.send
            .blocking_send(std::mem::take(&mut self.bytes))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the mount upload ended"))
    }
}
impl Write for Parts {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        crate::error::check_cancelled().map_err(io::Error::other)?;
        let count = bytes.len().min(self.limit - self.bytes.len());
        self.bytes.extend_from_slice(&bytes[..count]);
        if self.bytes.len() == self.limit {
            self.send_part()?;
        }
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.send_part()
    }
}

struct Frame {
    directory: Dir,
    cursor: NamesCursor,
    parent: usize,
}
struct Count {
    files: i64,
    bytes: i64,
    directories: i64,
    reported: Instant,
}
impl Count {
    fn report(&mut self, progress: &dyn Progress, final_report: bool) {
        if final_report || self.reported.elapsed() >= crate::snapshot::PROGRESS_INTERVAL {
            progress.counting(self.files, self.bytes, self.directories);
            self.reported = Instant::now();
        }
    }
}

fn encode_tree(directory: Dir, output: &mut impl Write, progress: &dyn Progress) -> Result<()> {
    let handle = directory
        .metadata_handle()
        .map_err(|error| io_error("pinning mount metadata", error))?;
    let mut links = BTreeMap::new();
    write_entry(&handle, &[], &mut links, output)?;
    let mut stack = vec![Frame {
        cursor: directory
            .names_cursor()
            .map_err(|error| io_error("reading a mount directory", error))?,
        directory,
        parent: 0,
    }];
    let mut path = Vec::new();
    let mut count = Count {
        files: 0,
        bytes: 0,
        directories: 1,
        reported: Instant::now(),
    };
    while let Some(frame) = stack.last_mut() {
        crate::error::check_cancelled()?;
        let Some(name) = frame
            .cursor
            .next_name()
            .map_err(|error| io_error("reading a mount directory", error))?
        else {
            let parent = frame.parent;
            stack.pop();
            path.truncate(parent);
            continue;
        };
        let parent = path.len();
        if !path.is_empty() {
            path.push(b'/');
        }
        path.extend_from_slice(name.as_bytes());
        let handle = frame
            .directory
            .open_metadata(&name)
            .map_err(|error| io_error("pinning mount metadata", error))?;
        let (entry, meta) = write_entry(&handle, &path, &mut links, output)?;
        if meta.is_file() {
            count.files = count.files.checked_add(1).ok_or_else(|| {
                HostError::Unsupported("the mount file count cannot be represented".into())
            })?;
        }
        if matches!(entry.kind, Kind::File) {
            let mut file = frame
                .directory
                .open_read(&name)
                .map_err(|error| io_error("opening a mount file", error))?;
            let opened = file
                .metadata()
                .map_err(|error| io_error("checking a mount file", error))?;
            if (opened.dev(), opened.ino()) != (meta.dev(), meta.ino()) {
                return Err(HostError::Changed("a mount entry changed identity".into()));
            }
            copy_file(&mut file, entry.size, output, &mut count, progress)?;
        }
        if meta.is_dir() {
            let directory = frame
                .directory
                .open_child(&name)
                .map_err(|error| io_error("opening a mount directory", error))?;
            let opened = directory
                .stat_self()
                .map_err(|error| io_error("checking a mount directory", error))?;
            if (opened.dev, opened.ino) != (meta.dev(), meta.ino()) {
                return Err(HostError::Changed(
                    "a mount directory changed identity".into(),
                ));
            }
            count.directories = count.directories.checked_add(1).ok_or_else(|| {
                HostError::Unsupported("the mount directory count cannot be represented".into())
            })?;
            stack.push(Frame {
                cursor: directory
                    .names_cursor()
                    .map_err(|error| io_error("reading a mount directory", error))?,
                directory,
                parent,
            });
        } else {
            path.truncate(parent);
        }
        count.report(progress, false);
    }
    output
        .write_all(&0_u32.to_le_bytes())
        .map_err(|error| io_error("closing a mount stream", error))?;
    output
        .flush()
        .map_err(|error| io_error("flushing a mount stream", error))?;
    count.report(progress, true);
    Ok(())
}

fn copy_file(
    file: &mut File,
    size: u64,
    output: &mut impl Write,
    count: &mut Count,
    progress: &dyn Progress,
) -> Result<()> {
    let mut left = size;
    let mut buffer = vec![0; crate::blobs::READ_CHUNK_BYTES];
    while left > 0 {
        crate::error::check_cancelled()?;
        let length = usize::try_from(left.min(buffer.len() as u64))
            .map_err(|_| HostError::Unsupported("the file chunk cannot be represented".into()))?;
        let read = file
            .read(&mut buffer[..length])
            .map_err(|error| io_error("reading a mount file", error))?;
        if read == 0 {
            return Err(HostError::Changed(
                "a mount file changed while it was carried".into(),
            ));
        }
        output
            .write_all(&buffer[..read])
            .map_err(|error| io_error("streaming a mount file", error))?;
        left -= read as u64;
        count.bytes = count.bytes.checked_add(read as i64).ok_or_else(|| {
            HostError::Unsupported("the mount byte count cannot be represented".into())
        })?;
        count.report(progress, false);
    }
    Ok(())
}

fn write_entry(
    handle: &File,
    path: &[u8],
    links: &mut BTreeMap<(u64, u64), Vec<u8>>,
    output: &mut impl Write,
) -> Result<(Entry, Metadata)> {
    let meta = handle
        .metadata()
        .map_err(|error| io_error("reading pinned mount metadata", error))?;
    let file_type = meta.file_type();
    let mut link = None;
    let kind = if meta.is_dir() {
        Kind::Directory
    } else if file_type.is_symlink() {
        link = Some(
            rustix::fs::readlinkat(handle, "", Vec::new())
                .map_err(|error| native_error("reading a mount link", error))?
                .into_bytes(),
        );
        Kind::Link
    } else if meta.is_file() {
        if meta.nlink() > 1 {
            match links.get(&(meta.dev(), meta.ino())) {
                Some(first) => {
                    link = Some(first.clone());
                    Kind::HardLink
                }
                None => {
                    links.insert((meta.dev(), meta.ino()), path.to_vec());
                    Kind::File
                }
            }
        } else {
            Kind::File
        }
    } else if file_type.is_char_device() {
        Kind::Character
    } else if file_type.is_block_device() {
        Kind::Block
    } else if file_type.is_fifo() {
        Kind::Fifo
    } else if file_type.is_socket() {
        Kind::Socket
    } else {
        return Err(HostError::Unsupported(
            "the mount contains an unknown native entry".into(),
        ));
    };
    let entry = Entry {
        path: path.to_vec(),
        kind,
        size: if matches!(kind, Kind::File) {
            meta.size()
        } else {
            0
        },
        uid: meta.uid(),
        gid: meta.gid(),
        mode: meta.mode() & 0o7777,
        atime: meta.atime(),
        atime_ns: meta.atime_nsec(),
        mtime: meta.mtime(),
        mtime_ns: meta.mtime_nsec(),
        link,
        major: rustix::fs::major(meta.rdev()).into(),
        minor: rustix::fs::minor(meta.rdev()).into(),
        xattrs: attributes(handle)?,
    };
    let encoded =
        serde_json::to_vec(&entry).map_err(|error| HostError::Content(error.to_string()))?;
    if encoded.len() > HEADER_MAX {
        return Err(HostError::Unsupported(
            "the mount entry metadata exceeds the volume stream bound".into(),
        ));
    }
    if path.is_empty() {
        output
            .write_all(MAGIC)
            .map_err(|error| io_error("opening a mount stream", error))?;
    }
    output
        .write_all(
            &u32::try_from(encoded.len())
                .map_err(|_| {
                    HostError::Unsupported("the mount header cannot be represented".into())
                })?
                .to_le_bytes(),
        )
        .map_err(|error| io_error("streaming mount metadata", error))?;
    output
        .write_all(&encoded)
        .map_err(|error| io_error("streaming mount metadata", error))?;
    Ok((entry, meta))
}

fn native_error(action: &str, error: rustix::io::Errno) -> HostError {
    io_error(action, io::Error::from_raw_os_error(error.raw_os_error()))
}

fn attributes(handle: &File) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    // A procfs descriptor link refers to this pinned inode, including an O_PATH
    // symbolic-link handle. No mutable source pathname participates in the lookup.
    let path = format!("/proc/self/fd/{}", handle.as_raw_fd());
    let names = loop {
        crate::error::check_cancelled()?;
        let size = match rustix::fs::listxattr_query(&path) {
            Ok(size) => size,
            Err(rustix::io::Errno::NOTSUP) => return Ok(Vec::new()),
            Err(error) => return Err(native_error("reading mount attributes", error)),
        };
        if size == 0 {
            return Ok(Vec::new());
        }
        let mut names = vec![0; size];
        match rustix::fs::listxattr(&path, names.as_mut_slice()) {
            Ok(size) => {
                names.truncate(size);
                break names;
            }
            Err(rustix::io::Errno::RANGE) => continue,
            Err(error) => return Err(native_error("reading mount attributes", error)),
        }
    };
    let mut attributes = Vec::new();
    for name in names
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let value = loop {
            crate::error::check_cancelled()?;
            let size = rustix::fs::getxattr_query(&path, name)
                .map_err(|error| native_error("reading a mount attribute", error))?;
            let mut value = vec![0; size];
            match rustix::fs::getxattr(&path, name, value.as_mut_slice()) {
                Ok(size) => {
                    value.truncate(size);
                    break value;
                }
                Err(rustix::io::Errno::RANGE) => continue,
                Err(error) => return Err(native_error("reading a mount attribute", error)),
            }
        };
        attributes.push((name.to_vec(), value));
    }
    Ok(attributes)
}
