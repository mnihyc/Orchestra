//! What an archive held in the store contains, listed and read one entry at a time without unpacking it.
//!
//! A zip, and a tar with or without gzip, zstd, xz or bzip2 compression, is listed from the blob
//! itself: its names, sizes and dates, one directory at a time, directories first. One entry of it can
//! be read, up to the preview size. Nothing is written anywhere, and nothing in an archive is run or
//! followed: a name that climbs out of its directory is only a name in a list, a link is only a link,
//! and an archive that asks for a password is listed where its format lets names be read and none of
//! its entries is opened. It is the reading that a machine does of an archive where it lies in a
//! workspace, done here for a file the store holds.
//!
//! The format is found from the first bytes and not from the name. A zip has an index at its end, so
//! its entry is found and read by asking the store for those bytes alone (ranges), however large the
//! zip is. A compressed tar has no index, so it is read through once; the reading stops at the limit
//! of entries or at the time limit, whichever comes first, and says so. The reading is blocking work
//! of a thread of its own, which asks the store for ranges through the runtime that started it.

use crate::blobs::Blobs;
use bytes::Bytes;
use chrono::{NaiveDate, TimeZone, Utc};
use orch_api_contract::workspace::{
    ArchiveCompression, ArchiveEntry, ArchiveEntryKind, ArchiveFormat, ArchiveListing,
};
use orch_blob_contract::range::{BlobMeta, ByteRange};
use orch_blob_contract::store::{BlobStore, read_all};
use orch_types::{Hash, Time};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long the reading of a compressed archive runs before it says it stopped short: the same
/// twenty seconds a machine gives itself.
pub const READ_TIME: Duration = Duration::from_secs(20);

/// How many bytes of the store one request for a range asks for: what the store itself reads in,
/// and enough that a reader that goes on from where it stopped asks seldom.
const WINDOW_BYTES: u64 = 256 * 1024;

/// The size of a tar header block, which also settles whether a stream is a tar.
const TAR_BLOCK_BYTES: usize = 512;

/// Where the `ustar` signature lies in a header block.
const TAR_MAGIC_OFFSET: usize = 257;

/// Where the checksum lies in a header block.
const TAR_CHECKSUM: std::ops::Range<usize> = 148..156;

/// How many bytes at the start of a file say what kind of archive it is.
const MAGIC_BYTES: usize = 8;

/// The most entries of a zip that are listed. The index of a zip is read whole into memory by the
/// reader, and an index that claims more entries than this is the work of someone who wants the
/// memory: a zip of this many entries is a hundred thousand files, more than a person looks through
/// one directory at a time.
pub const ZIP_ENTRIES_MAX: u64 = 100_000;

/// How far from the end of a zip its end-of-index record lies at most: the record and the longest
/// comment that follows it.
const ZIP_END_SEARCH_BYTES: u64 = 22 + 65_535;

/// The signature of the end-of-index record of a zip, and of the 64-bit locator before it and the
/// 64-bit record it points at.
const ZIP_END: [u8; 4] = *b"PK\x05\x06";
const ZIP64_LOCATOR: [u8; 4] = *b"PK\x06\x07";
const ZIP64_END: [u8; 4] = *b"PK\x06\x06";

/// Why an archive was not looked into.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArchiveError {
    /// The file is not an archive of a format that can be looked into.
    #[error("this file is not an archive that can be looked into")]
    NotAnArchive,
    /// The archive is damaged, or larger in its index than may be read.
    #[error("this archive cannot be read: {0}")]
    Unreadable(String),
    /// The entry is not in the archive.
    #[error("{0} is not in the archive")]
    NotFound(String),
    /// The entry is a directory or a link.
    #[error("{0} is not a file")]
    NotAFile(String),
    /// The archive asks for a password, so none of its entries is opened.
    #[error("{0} is encrypted, and no entry of an encrypted archive is opened")]
    Encrypted(String),
    /// The entry is larger than may be read.
    #[error("{entry} is larger than the {limit} bytes that may be read")]
    TooLarge {
        /// The entry.
        entry: String,
        /// The most bytes that may be read.
        limit: i64,
    },
    /// Reading a compressed archive took longer than it may.
    #[error("reading the archive took longer than it may")]
    Timeout,
    /// The store or the reading itself failed.
    #[error("{0}")]
    Failed(String),
}

fn unreadable(error: impl std::fmt::Display) -> ArchiveError {
    ArchiveError::Unreadable(error.to_string())
}

// ---------------------------------------------------------------- the blob as a file

/// A blob of the store read as a file: bytes by position, fetched from the store in windows. It is
/// for a thread that may block.
struct Source {
    store: Arc<dyn BlobStore>,
    hash: Hash,
    size: u64,
    runtime: tokio::runtime::Handle,
    position: u64,
    window_start: u64,
    window: Bytes,
}

impl Source {
    fn open(store: &Arc<dyn BlobStore>, meta: &BlobMeta, runtime: &tokio::runtime::Handle) -> Self {
        Self {
            store: Arc::clone(store),
            hash: meta.hash,
            size: u64::try_from(meta.size).unwrap_or(0),
            runtime: runtime.clone(),
            position: 0,
            window_start: 0,
            window: Bytes::new(),
        }
    }

    /// Reads the window that begins at the position.
    fn fetch(&mut self) -> std::io::Result<()> {
        let start = self.position;
        let end = (start + WINDOW_BYTES).min(self.size).saturating_sub(1);
        let range = ByteRange::Between {
            start: i64::try_from(start).unwrap_or(i64::MAX),
            end: i64::try_from(end).unwrap_or(i64::MAX),
        };
        let (store, hash) = (Arc::clone(&self.store), self.hash);
        let bytes = self
            .runtime
            .block_on(async move {
                let read = store.get(&hash, Some(range)).await?;
                read_all(read.body).await
            })
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        self.window_start = start;
        self.window = bytes;
        Ok(())
    }
}

impl Read for Source {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.position >= self.size || buffer.is_empty() {
            return Ok(0);
        }
        let held = self.position >= self.window_start
            && self.position < self.window_start + self.window.len() as u64;
        if !held {
            self.fetch()?;
        }
        let from = usize::try_from(self.position - self.window_start).unwrap_or(usize::MAX);
        let available = self.window.get(from..).unwrap_or_default();
        let count = available.len().min(buffer.len());
        buffer[..count].copy_from_slice(&available[..count]);
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for Source {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let target = match to {
            SeekFrom::Start(at) => i128::from(at),
            SeekFrom::End(by) => i128::from(self.size) + i128::from(by),
            SeekFrom::Current(by) => i128::from(self.position) + i128::from(by),
        };
        if target < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "a position before the start of the file",
            ));
        }
        self.position = u64::try_from(target).unwrap_or(u64::MAX);
        Ok(self.position)
    }
}

// ---------------------------------------------------------------- what the first bytes say

/// What an archive is, from its first bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Zip,
    Tar(Option<ArchiveCompression>),
}

fn decompress(
    source: Source,
    compression: Option<ArchiveCompression>,
) -> Result<Box<dyn Read>, ArchiveError> {
    Ok(match compression {
        None => Box::new(source),
        Some(ArchiveCompression::Gzip) => Box::new(flate2::read::MultiGzDecoder::new(source)),
        Some(ArchiveCompression::Zstd) => Box::new(
            zstd::stream::read::Decoder::new(source)
                .map_err(|error| ArchiveError::Failed(error.to_string()))?,
        ),
        Some(ArchiveCompression::Xz) => Box::new(liblzma::read::XzDecoder::new(source)),
        Some(ArchiveCompression::Bzip2) => Box::new(bzip2::read::BzDecoder::new(source)),
    })
}

fn is_tar_block(block: &[u8]) -> bool {
    if block.len() < TAR_BLOCK_BYTES {
        return false;
    }
    if &block[TAR_MAGIC_OFFSET..TAR_MAGIC_OFFSET + 5] == b"ustar" {
        return true;
    }
    // Older tars have no signature: the checksum of the block, with its own field as spaces, is.
    let stored = std::str::from_utf8(&block[TAR_CHECKSUM])
        .ok()
        .map(|text| text.trim_matches(|c: char| c == '\0' || c == ' '))
        .and_then(|text| u32::from_str_radix(text, 8).ok());
    let computed: u32 = block
        .iter()
        .enumerate()
        .map(|(at, byte)| {
            if TAR_CHECKSUM.contains(&at) {
                u32::from(b' ')
            } else {
                u32::from(*byte)
            }
        })
        .sum();
    block.iter().any(|byte| *byte != 0) && stored == Some(computed)
}

/// Finds what kind of archive the blob is; the reader is made anew for each use, so this takes the
/// means to make one.
fn shape_of(open: &dyn Fn() -> Source) -> Result<Shape, ArchiveError> {
    let mut magic = [0u8; MAGIC_BYTES];
    let count = fill(&mut open(), &mut magic)?;
    let magic = &magic[..count];
    if magic.starts_with(b"PK\x03\x04")
        || magic.starts_with(b"PK\x05\x06")
        || magic.starts_with(b"PK\x07\x08")
    {
        return Ok(Shape::Zip);
    }
    let compression = if magic.starts_with(&[0x1f, 0x8b]) {
        Some(ArchiveCompression::Gzip)
    } else if magic.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Some(ArchiveCompression::Zstd)
    } else if magic.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00]) {
        Some(ArchiveCompression::Xz)
    } else if magic.starts_with(b"BZh") {
        Some(ArchiveCompression::Bzip2)
    } else {
        None
    };
    let mut inner = decompress(open(), compression)?;
    let mut block = vec![0u8; TAR_BLOCK_BYTES];
    let filled = fill(&mut inner, &mut block).unwrap_or(0);
    block.truncate(filled);
    if is_tar_block(&block) {
        Ok(Shape::Tar(compression))
    } else {
        Err(ArchiveError::NotAnArchive)
    }
}

/// Reads until the buffer is full or the reader has no more.
fn fill(reader: &mut dyn Read, buffer: &mut [u8]) -> Result<usize, ArchiveError> {
    let mut filled = 0usize;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(ArchiveError::Failed(error.to_string())),
        }
    }
    Ok(filled)
}

/// How many entries the index of a zip claims, read from its end-of-index record without reading the
/// index: the 64-bit record when the ordinary one says so.
fn zip_entries<R: Read + Seek>(source: &mut R, size: u64) -> Result<u64, ArchiveError> {
    let tail = size.min(ZIP_END_SEARCH_BYTES);
    source
        .seek(SeekFrom::Start(size - tail))
        .map_err(unreadable)?;
    let mut bytes = vec![0u8; usize::try_from(tail).unwrap_or(0)];
    source.read_exact(&mut bytes).map_err(unreadable)?;
    let at = bytes
        .windows(ZIP_END.len())
        .rposition(|window| window == ZIP_END)
        .ok_or_else(|| ArchiveError::Unreadable("the zip has no end of index".to_owned()))?;
    let record = bytes
        .get(at..at + 22)
        .ok_or_else(|| ArchiveError::Unreadable("the end of index is cut short".to_owned()))?;
    let total = u64::from(u16::from_le_bytes([record[10], record[11]]));
    if total != u64::from(u16::MAX) {
        return Ok(total);
    }
    // The count does not fit: the 64-bit record that the locator in front of this one points at has it.
    let locator = at
        .checked_sub(20)
        .and_then(|from| bytes.get(from..at))
        .filter(|locator| locator.starts_with(&ZIP64_LOCATOR))
        .ok_or_else(|| ArchiveError::Unreadable("the zip has no 64-bit end of index".to_owned()))?;
    let mut offset = [0u8; 8];
    offset.copy_from_slice(&locator[8..16]);
    source
        .seek(SeekFrom::Start(u64::from_le_bytes(offset)))
        .map_err(unreadable)?;
    let mut end = [0u8; 56];
    source.read_exact(&mut end).map_err(unreadable)?;
    if !end.starts_with(&ZIP64_END) {
        return Err(ArchiveError::Unreadable(
            "the 64-bit end of index is not where the zip says".to_owned(),
        ));
    }
    let mut count = [0u8; 8];
    count.copy_from_slice(&end[32..40]);
    Ok(u64::from_le_bytes(count))
}

/// Reads a ZIP's declared entry count from its bounded footer, without scanning or unpacking its data.
pub async fn preview_count(blobs: &Blobs, meta: BlobMeta) -> Result<i64, ArchiveError> {
    let store = blobs.store();
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let mut source = Source::open(&store, &meta, &runtime);
        let size = source.size;
        zip_entries(&mut source, size).map(|count| i64::try_from(count).unwrap_or(i64::MAX))
    })
    .await
    .map_err(|error| ArchiveError::Failed(error.to_string()))?
}

/// Refuses a zip whose index claims more entries than are looked into.
fn check_zip_entries(source: &mut Source) -> Result<(), ArchiveError> {
    let size = source.size;
    let claimed = zip_entries(source, size)?;
    if claimed > ZIP_ENTRIES_MAX {
        return Err(ArchiveError::Unreadable(format!(
            "the zip holds {claimed} entries, and one that holds more than {ZIP_ENTRIES_MAX} is not looked into"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------- listing

/// An entry's path as the listing writes it: no leading `./` or `/`, no trailing `/`.
fn clean(path: &str) -> &str {
    let mut path = path;
    loop {
        let next = path
            .strip_prefix("./")
            .or_else(|| path.strip_prefix('/'))
            .unwrap_or(path);
        if next.len() == path.len() {
            break;
        }
        path = next;
    }
    path.trim_end_matches('/')
}

fn time_of_unix(seconds: u64) -> Option<Time> {
    Utc.timestamp_opt(i64::try_from(seconds).ok()?, 0)
        .single()
        .map(Time::new)
}

fn time_of_dos(stamp: zip::DateTime) -> Option<Time> {
    let date = NaiveDate::from_ymd_opt(
        i32::from(stamp.year()),
        u32::from(stamp.month()),
        u32::from(stamp.day()),
    )?;
    let naive = date.and_hms_opt(
        u32::from(stamp.hour()),
        u32::from(stamp.minute()),
        u32::from(stamp.second()),
    )?;
    // A zip keeps the time on the wall of whoever made it, with no zone: it is shown as it is.
    Some(Time::new(Utc.from_utc_datetime(&naive)))
}

/// The children of one directory of an archive, gathered from entries that come in any order.
struct Children {
    inside: String,
    limit: usize,
    found: BTreeMap<String, ArchiveEntry>,
}

impl Children {
    fn new(inside: Option<&str>, limit: usize) -> Self {
        Self {
            inside: inside.map(clean).unwrap_or("").to_owned(),
            limit,
            found: BTreeMap::new(),
        }
    }

    /// Notes an entry; says whether more than the limit are known now.
    fn add(
        &mut self,
        raw: &str,
        kind: ArchiveEntryKind,
        size: Option<i64>,
        modified_at: Option<Time>,
    ) -> bool {
        let path = clean(raw);
        let below = if self.inside.is_empty() {
            path
        } else {
            match path
                .strip_prefix(self.inside.as_str())
                .and_then(|rest| rest.strip_prefix('/'))
            {
                Some(rest) => rest,
                None => return false,
            }
        };
        if below.is_empty() {
            return false;
        }
        let (name, deeper) = match below.split_once('/') {
            Some((name, _)) => (name, true),
            None => (below, false),
        };
        if name.is_empty() {
            return false;
        }
        let full = if self.inside.is_empty() {
            name.to_owned()
        } else {
            format!("{}/{name}", self.inside)
        };
        if deeper {
            // Something below it exists, so it is a directory, whether or not it has an entry of its own.
            self.found.entry(name.to_owned()).or_insert(ArchiveEntry {
                path: full,
                kind: ArchiveEntryKind::Dir,
                size: None,
                modified_at: None,
            });
        } else {
            let entry = ArchiveEntry {
                path: full,
                kind,
                size: if kind == ArchiveEntryKind::Dir {
                    None
                } else {
                    size
                },
                modified_at,
            };
            self.found.insert(name.to_owned(), entry);
        }
        self.found.len() > self.limit
    }

    fn finish(self) -> (Vec<ArchiveEntry>, bool) {
        let truncated = self.found.len() > self.limit;
        let mut entries: Vec<ArchiveEntry> = self.found.into_values().collect();
        entries.sort_by(|a, b| {
            let dirs_first =
                (a.kind != ArchiveEntryKind::Dir).cmp(&(b.kind != ArchiveEntryKind::Dir));
            dirs_first
                .then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
                .then_with(|| a.path.cmp(&b.path))
        });
        entries.truncate(self.limit);
        (entries, truncated)
    }
}

/// Reads the index of a zip into the children of a directory; says whether any entry asks for a
/// password. Only the index is read: `by_index_data` describes an entry from it, where an entry
/// opened for its bytes would have the local header, wherever it lies in the file, fetched from the
/// store for each of them.
fn list_zip(mut source: Source, children: &mut Children) -> Result<bool, ArchiveError> {
    check_zip_entries(&mut source)?;
    let archive = zip::ZipArchive::new(source).map_err(unreadable)?;
    let mut encrypted = false;
    let mut enough = false;
    for index in 0..archive.len() {
        let entry = archive.by_index_data(index).map_err(unreadable)?;
        encrypted |= entry.encrypted();
        if enough {
            continue;
        }
        let Ok(name) = entry.name() else { continue };
        let kind = if entry.is_dir() {
            ArchiveEntryKind::Dir
        } else if entry.is_symlink() {
            ArchiveEntryKind::Link
        } else {
            ArchiveEntryKind::File
        };
        let size = i64::try_from(entry.size()).ok();
        enough = children.add(
            &name,
            kind,
            size,
            entry.last_modified().and_then(time_of_dos),
        );
    }
    Ok(encrypted)
}

/// Reads a tar through into the children of a directory; says whether the time ran out.
fn list_tar(
    reader: Box<dyn Read>,
    children: &mut Children,
    deadline: Instant,
) -> Result<bool, ArchiveError> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive.entries().map_err(unreadable)?;
    for entry in entries {
        if Instant::now() >= deadline {
            return Ok(true);
        }
        let entry = entry.map_err(unreadable)?;
        let header = entry.header();
        let kind = match header.entry_type() {
            tar::EntryType::Directory => ArchiveEntryKind::Dir,
            tar::EntryType::Symlink | tar::EntryType::Link => ArchiveEntryKind::Link,
            tar::EntryType::Regular | tar::EntryType::Continuous | tar::EntryType::GNUSparse => {
                ArchiveEntryKind::File
            }
            _ => continue,
        };
        let path = entry
            .path()
            .map_err(unreadable)?
            .to_string_lossy()
            .into_owned();
        let size = header.size().ok().and_then(|size| i64::try_from(size).ok());
        let modified = header.mtime().ok().and_then(time_of_unix);
        if children.add(&path, kind, size, modified) {
            return Ok(false);
        }
    }
    Ok(false)
}

/// Lists the directory `inside` of the archive that is the blob `meta` (the top of it when `inside`
/// is absent): at most `limit` entries, directories first. A compressed tar is read for at most
/// `time`, and says when that was not enough.
pub async fn list(
    blobs: &Blobs,
    meta: BlobMeta,
    inside: Option<&str>,
    limit: usize,
    time: Duration,
) -> Result<ArchiveListing, ArchiveError> {
    let store = blobs.store();
    let runtime = tokio::runtime::Handle::current();
    let inside = inside.map(str::to_owned);
    tokio::task::spawn_blocking(move || {
        let open = || Source::open(&store, &meta, &runtime);
        let shape = shape_of(&open)?;
        let mut children = Children::new(inside.as_deref(), limit.max(1));
        let deadline = Instant::now() + time;
        let (format, compression, encrypted, timed_out) = match shape {
            Shape::Zip => {
                let encrypted = list_zip(open(), &mut children)?;
                (ArchiveFormat::Zip, None, encrypted, false)
            }
            Shape::Tar(compression) => {
                let reader = decompress(open(), compression)?;
                let timed_out = list_tar(reader, &mut children, deadline)?;
                (ArchiveFormat::Tar, compression, false, timed_out)
            }
        };
        let (entries, over_limit) = children.finish();
        Ok(ArchiveListing {
            format,
            compression,
            entries,
            truncated: over_limit || timed_out,
            encrypted,
        })
    })
    .await
    .map_err(|error| ArchiveError::Failed(error.to_string()))?
}

// ---------------------------------------------------------------- reading one entry

/// One archive entry offered to document import without unpacking a path on disk.
#[derive(Debug)]
pub struct DocumentSource {
    /// Its source path, used only as a label and stable entry identity.
    pub name: String,
    /// Bounded supported text bytes, absent for a refused entry.
    pub bytes: Option<Vec<u8>>,
    /// A per-entry refusal; no successful entry is silently skipped.
    pub error: Option<String>,
    /// Whether it is a file that a text of the archive may name (a picture, a film, a PDF, a sheet) and no text itself:
    /// it becomes a file of the document that shows it, and is no entry of the report.
    pub companion: bool,
}

pub(crate) fn import_path(name: &str) -> Result<String, &'static str> {
    if name.starts_with('/')
        || name.contains('\\')
        || name.as_bytes().get(1) == Some(&b':')
        || name.chars().any(char::is_control)
        || name.split('/').any(|part| part == "..")
    {
        return Err("absolute, escaping and control-character paths are not imported");
    }
    let path = name
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>()
        .join("/");
    if path.is_empty() {
        return Err("an import entry needs a file name");
    }
    Ok(path)
}

/// Whether a file that is no text is of a kind that a text may name: any kind the system knows by its ending.
fn companion(name: &str) -> bool {
    orch_types::media::media_type(name, &[]) != orch_types::media::UNKNOWN
}

fn import_text(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        ".md",
        ".markdown",
        ".txt",
        ".html",
        ".htm",
        ".xhtml",
        ".docx",
    ]
    .iter()
    .any(|ending| name.ends_with(ending))
}

/// Reads supported entries of one archive within the existing entry and total expanded-byte bounds.
/// Raw paths and links are validated before any content is accepted; nothing is extracted on disk.
pub async fn documents(
    blobs: &Blobs,
    meta: BlobMeta,
    most: usize,
    expanded_limit: u64,
) -> Result<Vec<DocumentSource>, ArchiveError> {
    let store = blobs.store();
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let open = || Source::open(&store, &meta, &runtime);
        let shape = shape_of(&open)?;
        let deadline = Instant::now() + READ_TIME;
        let mut used = 0_u64;
        let mut names = std::collections::HashSet::new();
        let mut out = Vec::new();
        let mut take = |name: String,
                        regular: bool,
                        declared: u64,
                        reader: &mut dyn Read|
         -> Result<(), ArchiveError> {
            if Instant::now() >= deadline {
                return Err(ArchiveError::Timeout);
            }
            let safe = import_path(&name);
            let failure = match &safe {
                Err(reason) => Some((*reason).to_owned()),
                Ok(path) if !names.insert(path.clone()) => {
                    Some("duplicate archive entry name".to_owned())
                }
                Ok(_) if !regular => {
                    Some("links, encrypted files and non-file entries are not imported".to_owned())
                }
                Ok(path) if !import_text(path) && !companion(path) => {
                    Some("only Markdown, HTML, Word and plain text entries are imported".to_owned())
                }
                Ok(_) => None,
            };
            if declared > expanded_limit.saturating_sub(used) {
                return Err(too_large(&name, expanded_limit));
            }
            if let Some(error) = failure {
                used = used.saturating_add(declared);
                out.push(DocumentSource {
                    name,
                    bytes: None,
                    error: Some(error),
                    companion: false,
                });
            } else {
                let bytes = read_bounded(reader, &name, expanded_limit.saturating_sub(used))?;
                used = used.saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
                let name = safe.map_err(unreadable)?;
                out.push(DocumentSource {
                    companion: !import_text(&name),
                    name,
                    bytes: Some(bytes),
                    error: None,
                });
            }
            Ok(())
        };
        match shape {
            Shape::Zip => {
                let mut source = open();
                check_zip_entries(&mut source)?;
                let mut archive = zip::ZipArchive::new(source).map_err(unreadable)?;
                if archive.len() > most {
                    return Err(unreadable(format!(
                        "an import archive holds at most {most} entries"
                    )));
                }
                for index in 0..archive.len() {
                    let entry = archive.by_index_data(index).map_err(unreadable)?;
                    let name = entry.name().map_err(unreadable)?.to_string();
                    if entry.is_dir() {
                        if let Err(error) = import_path(&name) {
                            take(name, false, 0, &mut std::io::empty())?;
                            let _ = error;
                        }
                        continue;
                    }
                    let regular = !entry.is_symlink() && !entry.encrypted();
                    let declared = entry.size();
                    if !regular {
                        take(name, false, declared, &mut std::io::empty())?;
                        continue;
                    }
                    let mut entry = archive.by_index(index).map_err(unreadable)?;
                    take(name, true, declared, &mut entry)?;
                }
            }
            Shape::Tar(compression) => {
                let mut archive = tar::Archive::new(decompress(open(), compression)?);
                for (index, entry) in archive.entries().map_err(unreadable)?.enumerate() {
                    if index >= most {
                        return Err(unreadable(format!(
                            "an import archive holds at most {most} entries"
                        )));
                    }
                    let mut entry = entry.map_err(unreadable)?;
                    let name = entry
                        .path()
                        .map_err(unreadable)?
                        .to_string_lossy()
                        .into_owned();
                    if entry.header().entry_type().is_dir() {
                        if import_path(&name).is_err() {
                            take(name, false, 0, &mut std::io::empty())?;
                        }
                        continue;
                    }
                    let regular = entry.header().entry_type().is_file();
                    let declared = entry.size();
                    take(name, regular, declared, &mut entry)?;
                }
            }
        }
        Ok(out)
    })
    .await
    .map_err(|error| ArchiveError::Failed(error.to_string()))?
}

fn too_large(entry: &str, limit: u64) -> ArchiveError {
    ArchiveError::TooLarge {
        entry: entry.to_owned(),
        limit: i64::try_from(limit).unwrap_or(i64::MAX),
    }
}

fn read_bounded(reader: &mut dyn Read, entry: &str, limit: u64) -> Result<Vec<u8>, ArchiveError> {
    let mut bytes = Vec::new();
    // One byte more than the limit is read, so that an entry that is larger than it said is caught.
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| ArchiveError::Failed(error.to_string()))?;
    if bytes.len() as u64 > limit {
        return Err(too_large(entry, limit));
    }
    Ok(bytes)
}

fn read_zip(mut source: Source, wanted: &str, limit: u64) -> Result<Vec<u8>, ArchiveError> {
    check_zip_entries(&mut source)?;
    let mut archive = zip::ZipArchive::new(source).map_err(unreadable)?;
    let mut found = None;
    for index in 0..archive.len() {
        let entry = archive.by_index_data(index).map_err(unreadable)?;
        if entry.name().is_ok_and(|name| clean(&name) == wanted) {
            if entry.encrypted() {
                return Err(ArchiveError::Encrypted(wanted.to_owned()));
            }
            if entry.is_dir() || entry.is_symlink() {
                return Err(ArchiveError::NotAFile(wanted.to_owned()));
            }
            if entry.size() > limit {
                return Err(too_large(wanted, limit));
            }
            found = Some(index);
            break;
        }
    }
    let index = found.ok_or_else(|| ArchiveError::NotFound(wanted.to_owned()))?;
    let mut entry = archive.by_index(index).map_err(unreadable)?;
    read_bounded(&mut entry, wanted, limit)
}

fn read_tar(
    reader: Box<dyn Read>,
    wanted: &str,
    limit: u64,
    deadline: Instant,
) -> Result<Vec<u8>, ArchiveError> {
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries().map_err(unreadable)? {
        if Instant::now() >= deadline {
            return Err(ArchiveError::Timeout);
        }
        let mut entry = entry.map_err(unreadable)?;
        let name = entry
            .path()
            .map_err(unreadable)?
            .to_string_lossy()
            .into_owned();
        if clean(&name) != wanted {
            continue;
        }
        if !matches!(
            entry.header().entry_type(),
            tar::EntryType::Regular | tar::EntryType::Continuous
        ) {
            return Err(ArchiveError::NotAFile(wanted.to_owned()));
        }
        let size = entry.header().size().unwrap_or(0);
        if size > limit {
            return Err(too_large(wanted, limit));
        }
        return read_bounded(&mut entry, wanted, limit);
    }
    Err(ArchiveError::NotFound(wanted.to_owned()))
}

/// Reads the entry `entry` of the archive that is the blob `meta`, at most `limit` bytes of it. An
/// entry that is larger, that asks for a password or that is not a file is refused.
pub async fn read_entry(
    blobs: &Blobs,
    meta: BlobMeta,
    entry: &str,
    limit: u64,
    time: Duration,
) -> Result<Vec<u8>, ArchiveError> {
    let store = blobs.store();
    let runtime = tokio::runtime::Handle::current();
    let entry = clean(entry).to_owned();
    tokio::task::spawn_blocking(move || {
        let open = || Source::open(&store, &meta, &runtime);
        match shape_of(&open)? {
            Shape::Zip => read_zip(open(), &entry, limit),
            Shape::Tar(compression) => {
                let reader = decompress(open(), compression)?;
                read_tar(reader, &entry, limit, Instant::now() + time)
            }
        }
    })
    .await
    .map_err(|error| ArchiveError::Failed(error.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use orch_blob_contract::store::{BlobError, BlobRead, ByteStream, Stored};
    use std::io::Write;
    use std::sync::atomic::{AtomicI64, Ordering};

    /// A store that counts the bytes it hands out.
    struct Counting {
        inner: Arc<dyn BlobStore>,
        handed: AtomicI64,
    }

    #[async_trait]
    impl BlobStore for Counting {
        async fn put(
            &self,
            hash: &Hash,
            size: Option<i64>,
            content: ByteStream,
        ) -> Result<Stored, BlobError> {
            self.inner.put(hash, size, content).await
        }

        async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
            let read = self.inner.get(hash, range).await?;
            self.handed.fetch_add(read.range.len, Ordering::SeqCst);
            Ok(read)
        }

        async fn exists(&self, hash: &Hash) -> Result<bool, BlobError> {
            self.inner.exists(hash).await
        }

        async fn size(&self, hash: &Hash) -> Result<Option<i64>, BlobError> {
            self.inner.size(hash).await
        }

        async fn delete(&self, hash: &Hash) -> Result<bool, BlobError> {
            self.inner.delete(hash).await
        }
    }

    async fn stored(blobs: &Blobs, bytes: Vec<u8>) -> BlobMeta {
        blobs.put_bytes(Bytes::from(bytes)).await.unwrap()
    }

    fn zip_of(files: &[(&str, &[u8])], method: zip::CompressionMethod) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().compression_method(method);
        for (name, content) in files {
            writer.start_file(*name, options).unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn tar_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(1_700_000_000);
            header.set_cksum();
            builder.append_data(&mut header, name, *content).unwrap();
        }
        builder.into_inner().unwrap()
    }

    const TIME: Duration = Duration::from_secs(20);

    #[tokio::test(flavor = "multi_thread")]
    async fn one_entry_of_a_large_zip_is_read_by_asking_the_store_for_a_small_part_of_it() {
        // Two hundred files of a hundred kibibytes that do not compress: twenty mebibytes.
        let files: Vec<(String, Vec<u8>)> = (0..200u32)
            .map(|file| {
                let content: Vec<u8> = (0..100 * 1024u32)
                    .map(|at| {
                        u8::try_from(((at.wrapping_mul(2_654_435_761) ^ file) >> 13) & 0xff)
                            .unwrap()
                    })
                    .collect();
                (format!("data/file-{file:03}.bin"), content)
            })
            .collect();
        let borrowed: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(name, content)| (name.as_str(), content.as_slice()))
            .collect();
        let zip = zip_of(&borrowed, zip::CompressionMethod::Stored);
        let size = i64::try_from(zip.len()).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let counting = Arc::new(Counting {
            inner: Blobs::in_memory().store(),
            handed: AtomicI64::new(0),
        });
        let blobs = Blobs::new(counting.clone(), directory.path());
        let meta = stored(&blobs, zip).await;
        assert_eq!(meta.size, size);

        let read = read_entry(&blobs, meta, "data/file-137.bin", 1 << 20, TIME)
            .await
            .unwrap();
        assert_eq!(read, files[137].1);
        let handed = counting.handed.load(Ordering::SeqCst);
        assert!(
            handed < size / 8,
            "{handed} bytes of {size} were asked of the store to read one entry"
        );
        // The directory is listed from the index alone.
        counting.handed.store(0, Ordering::SeqCst);
        let listing = list(&blobs, meta, Some("data"), 1_000, TIME).await.unwrap();
        assert_eq!(listing.entries.len(), 200);
        assert!(counting.handed.load(Ordering::SeqCst) < size / 8);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_tar_is_found_in_every_compression_by_its_first_bytes_and_not_by_its_name() {
        let tar = tar_of(&[
            ("a/one.txt", b"one"),
            ("a/two.txt", b"two"),
            ("three.txt", b"three"),
        ]);
        let compressed: Vec<(Option<ArchiveCompression>, Vec<u8>)> = vec![
            (None, tar.clone()),
            (Some(ArchiveCompression::Gzip), {
                let mut out =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
                out.write_all(&tar).unwrap();
                out.finish().unwrap()
            }),
            (
                Some(ArchiveCompression::Zstd),
                zstd::encode_all(tar.as_slice(), 1).unwrap(),
            ),
            (Some(ArchiveCompression::Xz), {
                let mut out = liblzma::write::XzEncoder::new(Vec::new(), 1);
                out.write_all(&tar).unwrap();
                out.finish().unwrap()
            }),
            (Some(ArchiveCompression::Bzip2), {
                let mut out = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
                out.write_all(&tar).unwrap();
                out.finish().unwrap()
            }),
        ];
        let blobs = Blobs::in_memory();
        for (compression, bytes) in compressed {
            let meta = stored(&blobs, bytes).await;
            let top = list(&blobs, meta, None, 100, TIME).await.unwrap();
            assert_eq!(top.format, ArchiveFormat::Tar);
            assert_eq!(top.compression, compression);
            let names: Vec<&str> = top
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect();
            assert_eq!(
                names,
                ["a", "three.txt"],
                "{compression:?}: directories first"
            );
            let inside = list(&blobs, meta, Some("a/"), 100, TIME).await.unwrap();
            let names: Vec<&str> = inside
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect();
            assert_eq!(names, ["a/one.txt", "a/two.txt"], "{compression:?}");
            assert_eq!(
                read_entry(&blobs, meta, "/./a/two.txt", 100, TIME)
                    .await
                    .unwrap(),
                b"two",
                "{compression:?}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_file_that_is_no_archive_is_refused_and_so_is_a_zip_that_is_cut_short() {
        let blobs = Blobs::in_memory();
        for bytes in [
            b"just some words, no archive in them".to_vec(),
            Vec::new(),
            vec![0u8; 4096],
        ] {
            let meta = stored(&blobs, bytes).await;
            assert_eq!(
                list(&blobs, meta, None, 10, TIME).await.unwrap_err(),
                ArchiveError::NotAnArchive
            );
            assert_eq!(
                read_entry(&blobs, meta, "x", 10, TIME).await.unwrap_err(),
                ArchiveError::NotAnArchive
            );
        }
        let zip = zip_of(&[("a.txt", b"a")], zip::CompressionMethod::Stored);
        let meta = stored(&blobs, zip[..zip.len() / 2].to_vec()).await;
        assert!(matches!(
            list(&blobs, meta, None, 10, TIME).await.unwrap_err(),
            ArchiveError::Unreadable(_)
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_entry_is_read_up_to_the_limit_and_a_name_that_is_not_a_file_is_not_read() {
        let blobs = Blobs::in_memory();
        let zip = zip_of(
            &[("small.txt", b"1234"), ("big.txt", &[b'x'; 100])],
            zip::CompressionMethod::Deflated,
        );
        let meta = stored(&blobs, zip).await;
        assert_eq!(
            read_entry(&blobs, meta, "small.txt", 4, TIME)
                .await
                .unwrap(),
            b"1234"
        );
        assert_eq!(
            read_entry(&blobs, meta, "big.txt", 50, TIME)
                .await
                .unwrap_err(),
            ArchiveError::TooLarge {
                entry: "big.txt".to_owned(),
                limit: 50
            }
        );
        assert_eq!(
            read_entry(&blobs, meta, "nope.txt", 50, TIME)
                .await
                .unwrap_err(),
            ArchiveError::NotFound("nope.txt".to_owned())
        );
        // The same of a tar, which is read through.
        let tar = tar_of(&[("small.txt", b"1234"), ("big.txt", &[b'x'; 100])]);
        let meta = stored(&blobs, tar).await;
        assert_eq!(
            read_entry(&blobs, meta, "big.txt", 50, TIME)
                .await
                .unwrap_err(),
            ArchiveError::TooLarge {
                entry: "big.txt".to_owned(),
                limit: 50
            }
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_tar_that_is_read_for_longer_than_may_be_says_so() {
        let blobs = Blobs::in_memory();
        let tar = tar_of(&[("a.txt", b"a"), ("b.txt", b"b")]);
        let meta = stored(&blobs, tar).await;
        let listing = list(&blobs, meta, None, 10, Duration::ZERO).await.unwrap();
        assert!(listing.truncated, "what was read so far is all there is");
        assert_eq!(
            read_entry(&blobs, meta, "b.txt", 10, Duration::ZERO)
                .await
                .unwrap_err(),
            ArchiveError::Timeout
        );
    }

    #[test]
    fn a_zip_that_claims_more_entries_than_are_looked_into_is_known_by_its_end_record() {
        // A 64-bit end record that says a billion entries, found through the locator before the end record.
        let mut zip = vec![0u8; 100];
        let record_at = zip.len() as u64;
        let mut record = Vec::new();
        record.extend_from_slice(&ZIP64_END);
        record.extend_from_slice(&44u64.to_le_bytes());
        record.extend_from_slice(&[45, 0, 45, 0]);
        record.extend_from_slice(&0u32.to_le_bytes());
        record.extend_from_slice(&0u32.to_le_bytes());
        record.extend_from_slice(&1_000_000_000u64.to_le_bytes());
        record.extend_from_slice(&1_000_000_000u64.to_le_bytes());
        record.extend_from_slice(&0u64.to_le_bytes());
        record.extend_from_slice(&0u64.to_le_bytes());
        zip.extend_from_slice(&record);
        zip.extend_from_slice(&ZIP64_LOCATOR);
        zip.extend_from_slice(&0u32.to_le_bytes());
        zip.extend_from_slice(&record_at.to_le_bytes());
        zip.extend_from_slice(&1u32.to_le_bytes());
        let mut end = Vec::new();
        end.extend_from_slice(&ZIP_END);
        end.extend_from_slice(&[0; 4]);
        end.extend_from_slice(&u16::MAX.to_le_bytes());
        end.extend_from_slice(&u16::MAX.to_le_bytes());
        end.extend_from_slice(&u32::MAX.to_le_bytes());
        end.extend_from_slice(&u32::MAX.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&end);
        let size = zip.len() as u64;
        let claimed = zip_entries(&mut std::io::Cursor::new(zip), size).unwrap();
        assert_eq!(claimed, 1_000_000_000);
        assert!(claimed > ZIP_ENTRIES_MAX);

        // An ordinary zip says how many it has in its own end record.
        let ordinary = zip_of(
            &[("a", b"a"), ("b", b"b"), ("c", b"c")],
            zip::CompressionMethod::Stored,
        );
        let size = ordinary.len() as u64;
        assert_eq!(
            zip_entries(&mut std::io::Cursor::new(ordinary), size).unwrap(),
            3
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_zip_with_a_password_is_listed_and_none_of_its_entries_is_opened() {
        let mut zip = zip_of(&[("secret.txt", b"hidden")], zip::CompressionMethod::Stored);
        // Marks every entry as asking for a password: bit 0 of the flags in each header.
        let mut at = 0;
        while let Some(found) = zip[at..]
            .windows(4)
            .position(|w| w == b"PK\x03\x04" || w == b"PK\x01\x02")
        {
            let header = at + found;
            let flags = if &zip[header..header + 4] == b"PK\x03\x04" {
                header + 6
            } else {
                header + 8
            };
            zip[flags] |= 1;
            at = header + 4;
        }
        let blobs = Blobs::in_memory();
        let meta = stored(&blobs, zip).await;
        let listing = list(&blobs, meta, None, 10, TIME).await.unwrap();
        assert!(listing.encrypted);
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(
            read_entry(&blobs, meta, "secret.txt", 100, TIME)
                .await
                .unwrap_err(),
            ArchiveError::Encrypted("secret.txt".to_owned())
        );
    }
}
