//! What an archive holds, read where it lies and without unpacking it.
//!
//! A zip, and a tar with or without gzip, zstd, xz or bzip2 compression, is listed from the file
//! itself: its names, sizes and dates, one directory at a time, directories first. One entry of
//! it can be read, up to the preview size, for a viewer. Nothing is written into the workspace or
//! anywhere else, and nothing in an archive is run or followed: a name that climbs out of its
//! directory is only a name in a list, a link is only a link, and an archive that asks for a
//! password is listed where its format lets names be read and none of its entries is opened.
//!
//! The format is found from the first bytes and not from the extension. A compressed tar has no
//! index, so it is read through once, and the reading stops at the limit of entries or at the
//! time limit, whichever comes first, and says so.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek};
use std::sync::Arc;
use std::time::Instant;

use chrono::{NaiveDate, TimeZone, Utc};
use orch_api_contract::workspace::{
    ArchiveCompression, ArchiveEntry, ArchiveEntryKind, ArchiveFormat, ArchiveListing,
};
use orch_types::Time;

use crate::error::{HostError, Result};
use crate::files::Files;
use crate::paths::RelPath;
use crate::platform::read_at;

/// The size of a tar header block, which also settles whether a stream is a tar.
const TAR_BLOCK_BYTES: usize = 512;

/// Where the `ustar` signature lies in a header block.
const TAR_MAGIC_OFFSET: usize = 257;

/// Where the checksum lies in a header block, and how long it is.
const TAR_CHECKSUM: std::ops::Range<usize> = 148..156;

/// How many bytes at the start of a file say what kind of archive it is.
const MAGIC_BYTES: usize = 8;

/// What an archive is, from its first bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Zip,
    Tar(Option<ArchiveCompression>),
}

/// Reads an open file from the start by position, so that any number of readers of one file are
/// independent of each other (a duplicated descriptor would share its cursor).
struct Positional {
    file: Arc<File>,
    offset: u64,
}

impl Read for Positional {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = read_at(&self.file, buffer, self.offset)?;
        self.offset += count as u64;
        Ok(count)
    }
}

fn decompress(file: &Arc<File>, compression: Option<ArchiveCompression>) -> Result<Box<dyn Read>> {
    let source = Positional {
        file: Arc::clone(file),
        offset: 0,
    };
    let reader: Box<dyn Read> = match compression {
        None => Box::new(source),
        Some(ArchiveCompression::Gzip) => Box::new(flate2::read::MultiGzDecoder::new(source)),
        Some(ArchiveCompression::Zstd) => Box::new(
            zstd::stream::read::Decoder::new(source)
                .map_err(|error| HostError::io("opening a zstd stream", error))?,
        ),
        Some(ArchiveCompression::Xz) => Box::new(liblzma::read::XzDecoder::new(source)),
        Some(ArchiveCompression::Bzip2) => Box::new(bzip2::read::BzDecoder::new(source)),
    };
    Ok(reader)
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

fn shape_of(file: &Arc<File>) -> Result<Shape> {
    let mut magic = [0u8; MAGIC_BYTES];
    let count = read_at(file, &mut magic, 0)
        .map_err(|error| HostError::io("reading the start of an archive", error))?;
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
    let mut inner = decompress(file, compression)?;
    let mut block = vec![0u8; TAR_BLOCK_BYTES];
    let mut filled = 0usize;
    while filled < TAR_BLOCK_BYTES {
        match inner.read(&mut block[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    block.truncate(filled);
    if is_tar_block(&block) {
        Ok(Shape::Tar(compression))
    } else {
        Err(HostError::Unsupported(
            "this file is not an archive that can be looked into".to_owned(),
        ))
    }
}

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

fn unreadable(error: impl std::fmt::Display) -> HostError {
    HostError::Unsupported(format!("this archive cannot be read: {error}"))
}

fn list_zip(file: &File, children: &mut Children) -> Result<bool> {
    let mut archive = zip::ZipArchive::new(file).map_err(unreadable)?;
    let mut encrypted = false;
    let mut enough = false;
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(unreadable)?;
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

fn list_tar(reader: Box<dyn Read>, children: &mut Children, deadline: Instant) -> Result<bool> {
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

impl Files {
    /// Lists the directory `inside` of the archive at `path` (the top of it when `inside` is
    /// absent): at most `limit` entries, directories first.
    pub fn archive_list(
        &self,
        path: &RelPath,
        inside: Option<&str>,
        limit: usize,
    ) -> Result<ArchiveListing> {
        let opened = self.open_regular(path)?;
        let file = &opened.file;
        let shape = shape_of(file)?;
        let mut children = Children::new(inside, limit.max(1));
        let deadline = Instant::now() + self.limits().archive_time;
        let (format, compression, encrypted, timed_out) = match shape {
            Shape::Zip => {
                let encrypted = list_zip(file.as_ref(), &mut children)?;
                (ArchiveFormat::Zip, None, encrypted, false)
            }
            Shape::Tar(compression) => {
                let reader = decompress(file, compression)?;
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
    }

    /// Reads the entry `entry` of the archive at `path`, at most the preview size of it. An entry
    /// that is larger, that asks for a password or is not a file is refused.
    pub fn archive_read(&self, path: &RelPath, entry: &str) -> Result<Vec<u8>> {
        let opened = self.open_regular(path)?;
        let file = &opened.file;
        let limit = self.limits().preview_bytes;
        let wanted = clean(entry);
        match shape_of(file)? {
            Shape::Zip => read_zip(file.as_ref(), wanted, limit),
            Shape::Tar(compression) => {
                let reader = decompress(file, compression)?;
                read_tar(
                    reader,
                    wanted,
                    limit,
                    Instant::now() + self.limits().archive_time,
                )
            }
        }
    }
}

fn too_large(what: &str, size: u64, limit: u64) -> HostError {
    HostError::TooLarge {
        what: what.to_owned(),
        size: i64::try_from(size).unwrap_or(i64::MAX),
        limit: i64::try_from(limit).unwrap_or(i64::MAX),
    }
}

fn read_bounded(reader: &mut dyn Read, what: &str, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    // One byte more than the limit is read, so that an entry that is larger than it said is caught.
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| HostError::io(format!("reading {what} from an archive"), error))?;
    if bytes.len() as u64 > limit {
        return Err(too_large(what, bytes.len() as u64, limit));
    }
    Ok(bytes)
}

fn read_zip<R: Read + Seek>(source: R, wanted: &str, limit: u64) -> Result<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(source).map_err(unreadable)?;
    let mut found = None;
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(unreadable)?;
        if entry.name().is_ok_and(|name| clean(&name) == wanted) {
            if entry.encrypted() {
                return Err(HostError::Unsupported(format!(
                    "{wanted} is encrypted, and no entry of an encrypted archive is opened"
                )));
            }
            if entry.is_dir() || entry.is_symlink() {
                return Err(HostError::Unsupported(format!("{wanted} is not a file")));
            }
            if entry.size() > limit {
                return Err(too_large(wanted, entry.size(), limit));
            }
            found = Some(index);
            break;
        }
    }
    let index =
        found.ok_or_else(|| HostError::NotFound(format!("{wanted} is not in the archive")))?;
    let mut entry = archive.by_index(index).map_err(unreadable)?;
    read_bounded(&mut entry, wanted, limit)
}

fn read_tar(reader: Box<dyn Read>, wanted: &str, limit: u64, deadline: Instant) -> Result<Vec<u8>> {
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries().map_err(unreadable)? {
        if Instant::now() >= deadline {
            return Err(HostError::Timeout(
                "reading the archive took longer than it may".to_owned(),
            ));
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
            return Err(HostError::Unsupported(format!("{wanted} is not a file")));
        }
        let size = entry.header().size().unwrap_or(0);
        if size > limit {
            return Err(too_large(wanted, size, limit));
        }
        return read_bounded(&mut entry, wanted, limit);
    }
    Err(HostError::NotFound(format!(
        "{wanted} is not in the archive"
    )))
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crate::environment::{GateState, Gates, Limits};
    use crate::ignore::IgnoreRules;
    use crate::index::HashCache;
    use crate::paths::{PathStyle, Workspace};
    use std::io::Write;
    use std::sync::Arc;

    fn files_with(root: &std::path::Path, limits: Limits) -> Files {
        let ws = Workspace::open(root, &[], PathStyle::Posix, true).unwrap();
        Files::workspace(
            ws,
            Arc::new(IgnoreRules::new(&[], true)),
            Arc::new(HashCache::new()),
            limits,
            Arc::new(GateState::new(Gates::OPEN)),
        )
    }

    fn path(text: &str) -> RelPath {
        RelPath::parse(text, PathStyle::Posix).unwrap()
    }

    /// A tar holding a small tree: `docs/` (no entry of its own for `docs/img/`), a link and a file at the top.
    fn tar_bytes() -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut add_file = |name: &str, content: &[u8]| {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(1_700_000_000);
            header.set_cksum();
            builder.append_data(&mut header, name, content).unwrap();
        };
        add_file("top.txt", b"top level");
        add_file("docs/readme.md", b"# readme");
        add_file("docs/img/logo.png", b"not really a png");
        add_file("docs/notes/a.txt", b"a");
        let mut dir = tar::Header::new_gnu();
        dir.set_entry_type(tar::EntryType::Directory);
        dir.set_size(0);
        dir.set_mode(0o755);
        dir.set_mtime(1_700_000_100);
        dir.set_cksum();
        builder
            .append_data(&mut dir, "docs/", std::io::empty())
            .unwrap();
        let mut link = tar::Header::new_gnu();
        link.set_entry_type(tar::EntryType::Symlink);
        link.set_size(0);
        link.set_cksum();
        builder.append_link(&mut link, "latest", "top.txt").unwrap();
        builder.into_inner().unwrap()
    }

    fn zip_bytes(encrypted: bool) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("top.txt", options).unwrap();
        writer.write_all(b"top level").unwrap();
        writer.add_directory("docs/", options).unwrap();
        writer
            .start_file(
                "docs/readme.md",
                options.compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(&b"# readme ".repeat(100)).unwrap();
        writer.start_file("docs/deep/er/file.txt", options).unwrap();
        writer.write_all(b"deep").unwrap();
        let mut bytes = writer.finish().unwrap().into_inner();
        if encrypted {
            // Mark every entry as asking for a password: bit 0 of the flags in each header.
            let mut at = 0;
            while let Some(found) = bytes[at..]
                .windows(4)
                .position(|w| w == b"PK\x03\x04" || w == b"PK\x01\x02")
            {
                let header = at + found;
                let flags = if &bytes[header..header + 4] == b"PK\x03\x04" {
                    header + 6
                } else {
                    header + 8
                };
                bytes[flags] |= 1;
                at = header + 4;
            }
        }
        bytes
    }

    fn names(listing: &ArchiveListing) -> Vec<(&str, ArchiveEntryKind)> {
        listing
            .entries
            .iter()
            .map(|e| (e.path.as_str(), e.kind))
            .collect()
    }

    #[test]
    fn a_tar_is_listed_one_directory_at_a_time_without_unpacking_it() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.tar"), tar_bytes()).unwrap();
        let files = files_with(root.path(), Limits::default());
        let top = files.archive_list(&path("a.tar"), None, 100).unwrap();
        assert_eq!(top.format, ArchiveFormat::Tar);
        assert_eq!(top.compression, None);
        assert!(!top.encrypted && !top.truncated);
        use ArchiveEntryKind::{Dir, File, Link};
        assert_eq!(
            names(&top),
            [("docs", Dir), ("latest", Link), ("top.txt", File)],
            "directories first"
        );
        let docs = files
            .archive_list(&path("a.tar"), Some("docs"), 100)
            .unwrap();
        assert_eq!(
            names(&docs),
            [
                ("docs/img", Dir),
                ("docs/notes", Dir),
                ("docs/readme.md", File)
            ],
            "a directory with no entry of its own is still there"
        );
        let readme = docs
            .entries
            .iter()
            .find(|e| e.path == "docs/readme.md")
            .unwrap();
        assert_eq!(readme.size, Some(8));
        assert!(readme.modified_at.is_some());
        let logo = files
            .archive_list(&path("a.tar"), Some("docs/img/"), 100)
            .unwrap();
        assert_eq!(names(&logo), [("docs/img/logo.png", File)]);
        assert_eq!(
            std::fs::read_dir(root.path()).unwrap().count(),
            1,
            "nothing was written into the workspace"
        );
    }

    #[test]
    fn every_compression_of_a_tar_is_found_by_its_first_bytes_and_listed() {
        let root = tempfile::tempdir().unwrap();
        let tar = tar_bytes();
        let gz = {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(&tar).unwrap();
            encoder.finish().unwrap()
        };
        let zst = zstd::stream::encode_all(&tar[..], 3).unwrap();
        let xz = {
            let mut encoder = liblzma::write::XzEncoder::new(Vec::new(), 3);
            encoder.write_all(&tar).unwrap();
            encoder.finish().unwrap()
        };
        let bz = {
            let mut encoder =
                bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
            encoder.write_all(&tar).unwrap();
            encoder.finish().unwrap()
        };
        // The names say nothing: the first bytes do.
        for (name, bytes, compression) in [
            ("one", gz, ArchiveCompression::Gzip),
            ("two", zst, ArchiveCompression::Zstd),
            ("three", xz, ArchiveCompression::Xz),
            ("four", bz, ArchiveCompression::Bzip2),
        ] {
            std::fs::write(root.path().join(name), bytes).unwrap();
            let files = files_with(root.path(), Limits::default());
            let listing = files.archive_list(&path(name), None, 100).unwrap();
            assert_eq!(listing.format, ArchiveFormat::Tar, "{name}");
            assert_eq!(listing.compression, Some(compression), "{name}");
            assert_eq!(listing.entries.len(), 3, "{name}");
            let read = files.archive_read(&path(name), "docs/readme.md").unwrap();
            assert_eq!(read, b"# readme", "{name}");
        }
    }

    #[test]
    fn a_zip_is_listed_with_its_directories_and_one_entry_is_read_within_the_preview_size() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.zip"), zip_bytes(false)).unwrap();
        let files = files_with(root.path(), Limits::default());
        let top = files.archive_list(&path("a.zip"), None, 100).unwrap();
        assert_eq!(top.format, ArchiveFormat::Zip);
        assert!(!top.encrypted);
        use ArchiveEntryKind::{Dir, File};
        assert_eq!(names(&top), [("docs", Dir), ("top.txt", File)]);
        let deep = files
            .archive_list(&path("a.zip"), Some("docs/deep"), 100)
            .unwrap();
        assert_eq!(names(&deep), [("docs/deep/er", Dir)]);
        assert_eq!(
            files
                .archive_read(&path("a.zip"), "docs/deep/er/file.txt")
                .unwrap(),
            b"deep"
        );
        assert_eq!(
            files
                .archive_read(&path("a.zip"), "docs/readme.md")
                .unwrap()
                .len(),
            900,
            "a deflated entry is inflated"
        );
        assert!(matches!(
            files.archive_read(&path("a.zip"), "nope.txt"),
            Err(HostError::NotFound(_))
        ));
        assert!(matches!(
            files.archive_read(&path("a.zip"), "docs"),
            Err(HostError::Unsupported(_))
        ));
        // The same entry is refused when it is larger than the preview size.
        let small = files_with(
            root.path(),
            Limits {
                preview_bytes: 100,
                ..Limits::default()
            },
        );
        assert!(matches!(
            small.archive_read(&path("a.zip"), "docs/readme.md"),
            Err(HostError::TooLarge { .. })
        ));
        assert_eq!(
            small.archive_read(&path("a.zip"), "top.txt").unwrap(),
            b"top level"
        );
        assert_eq!(
            std::fs::read_dir(root.path()).unwrap().count(),
            1,
            "nothing was written into the workspace"
        );
    }

    #[test]
    fn an_entry_of_a_tar_is_read_within_the_preview_size_and_refused_beyond_it() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.tar"), tar_bytes()).unwrap();
        let files = files_with(root.path(), Limits::default());
        assert_eq!(
            files.archive_read(&path("a.tar"), "top.txt").unwrap(),
            b"top level"
        );
        assert_eq!(
            files
                .archive_read(&path("a.tar"), "./docs/readme.md")
                .unwrap(),
            b"# readme"
        );
        assert!(
            matches!(
                files.archive_read(&path("a.tar"), "latest"),
                Err(HostError::Unsupported(_))
            ),
            "a link is not read"
        );
        let small = files_with(
            root.path(),
            Limits {
                preview_bytes: 4,
                ..Limits::default()
            },
        );
        assert!(matches!(
            small.archive_read(&path("a.tar"), "top.txt"),
            Err(HostError::TooLarge { .. })
        ));
    }

    #[test]
    fn an_encrypted_zip_is_listed_and_none_of_it_is_opened() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("secret.zip"), zip_bytes(true)).unwrap();
        let files = files_with(root.path(), Limits::default());
        let listing = files.archive_list(&path("secret.zip"), None, 100).unwrap();
        assert!(listing.encrypted);
        assert_eq!(listing.entries.len(), 2, "the names are readable");
        for entry in ["top.txt", "docs/readme.md"] {
            assert!(
                matches!(
                    files.archive_read(&path("secret.zip"), entry),
                    Err(HostError::Unsupported(_))
                ),
                "{entry}"
            );
        }
    }

    #[test]
    fn an_archive_with_more_entries_than_the_limit_lists_the_limit_and_says_so() {
        let root = tempfile::tempdir().unwrap();
        let mut builder = tar::Builder::new(Vec::new());
        for n in 0..30 {
            let mut header = tar::Header::new_gnu();
            header.set_size(1);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("file{n:02}"), &b"x"[..])
                .unwrap();
        }
        std::fs::write(root.path().join("many.tar"), builder.into_inner().unwrap()).unwrap();
        let files = files_with(root.path(), Limits::default());
        let listing = files.archive_list(&path("many.tar"), None, 10).unwrap();
        assert_eq!(listing.entries.len(), 10);
        assert!(listing.truncated);
        let all = files.archive_list(&path("many.tar"), None, 30).unwrap();
        assert_eq!(all.entries.len(), 30);
        assert!(!all.truncated);
    }

    #[test]
    fn a_file_that_is_no_archive_or_a_damaged_one_is_refused_with_the_reason() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("plain.txt"), "just text, not an archive").unwrap();
        let mut broken = zip_bytes(false);
        broken.truncate(broken.len() - 40);
        std::fs::write(root.path().join("broken.zip"), broken).unwrap();
        let files = files_with(root.path(), Limits::default());
        match files.archive_list(&path("plain.txt"), None, 10) {
            Err(HostError::Unsupported(reason)) => assert!(reason.contains("not an archive")),
            other => panic!("{other:?}"),
        }
        match files.archive_list(&path("broken.zip"), None, 10) {
            Err(HostError::Unsupported(reason)) => {
                assert!(reason.contains("cannot be read"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_name_that_climbs_out_of_its_directory_is_only_a_name_in_the_list() {
        let root = tempfile::tempdir().unwrap();
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(4);
        header.set_mode(0o644);
        header.set_cksum();
        // The builder refuses to write such a name, so it is put in the header by hand.
        header.as_old_mut().name[..14].copy_from_slice(b"../../evil.txt");
        header.set_cksum();
        builder.append(&header, &b"evil"[..]).unwrap();
        std::fs::write(root.path().join("evil.tar"), builder.into_inner().unwrap()).unwrap();
        let files = files_with(root.path(), Limits::default());
        let listing = files.archive_list(&path("evil.tar"), None, 10).unwrap();
        assert_eq!(listing.entries.len(), 1);
        assert!(!root.path().parent().unwrap().join("evil.txt").exists());
        assert_eq!(
            files
                .archive_read(&path("evil.tar"), "../../evil.txt")
                .unwrap(),
            b"evil"
        );
    }
}
