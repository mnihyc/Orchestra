//! The native archive stays compatible while its manifest and payload are streamed through disk.

use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use orch_agent_contract::history::SessionArchive;
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};

use super::{ARCHIVE_MAGIC, HistoryError, find_transcript, uuid_like, version_in_reader};
use crate::config::{MANAGED_PROJECT_DIR, PORTABLE_ARCHIVE_CANDIDATE, TRANSCRIPT_FORMAT};

// One copy page, independent of the archive's length or number of files. Metadata tokens have a
// separate bound: even a fully escaped filesystem path fits comfortably in this page.
const PAGE_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize, Serialize)]
struct Entry {
    path: String,
    size: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct SourceEntry {
    entry: Entry,
    modified: Option<SystemTime>,
}

#[derive(Debug)]
struct Header {
    session_id: String,
    harness_version: Option<String>,
    payload_bytes: u64,
}

#[derive(Debug, Default)]
struct Work {
    entries: u64,
    copied_bytes: u64,
    largest_read: usize,
    largest_metadata_token: usize,
}

fn invalid(message: impl Into<String>) -> HistoryError {
    HistoryError::NotAnArchive(message.into())
}

fn json_error(error: serde_json::Error) -> HistoryError {
    invalid(error.to_string())
}

fn check_cancel(cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "native archive work was cancelled",
        ))
    } else {
        Ok(())
    }
}

struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce(&AtomicBool) -> Result<T, HistoryError> + Send + 'static,
) -> Result<T, HistoryError> {
    let cancel = Arc::new(AtomicBool::new(false));
    let _guard = CancelOnDrop(Arc::clone(&cancel));
    tokio::task::spawn_blocking(move || work(&cancel))
        .await
        .map_err(|error| io::Error::other(error.to_string()))?
}

pub(super) async fn export(
    home: &Path,
    session_id: &str,
    into: &Path,
) -> Result<SessionArchive, HistoryError> {
    let (home, id, into) = (
        home.to_path_buf(),
        session_id.to_owned(),
        into.to_path_buf(),
    );
    blocking(move |cancel| export_files(&home, &id, &into, cancel).map(|(archive, _)| archive))
        .await
}

pub(super) async fn import(
    home: &Path,
    managed: bool,
    archive: &SessionArchive,
    target_version: &str,
) -> Result<String, HistoryError> {
    let home = home.to_path_buf();
    let path = PathBuf::from(&archive.path);
    let version = target_version.to_owned();
    blocking(move |cancel| import_files(&home, managed, &path, &version, cancel).map(|(id, _)| id))
        .await
}

struct CancellableRead<'a, R> {
    input: R,
    cancel: &'a AtomicBool,
}

impl<R: Read> Read for CancellableRead<'_, R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        check_cancel(self.cancel)?;
        self.input.read(out)
    }
}

/// Copies exactly one captured file, never asking its reader for more than one page.
fn copy_file(
    source: &mut impl Read,
    target: &mut impl Write,
    mut left: u64,
    cancel: &AtomicBool,
    work: &mut Work,
) -> Result<(), HistoryError> {
    let mut page = [0_u8; PAGE_BYTES];
    while left > 0 {
        check_cancel(cancel)?;
        let wanted = usize::try_from(left.min(PAGE_BYTES as u64)).unwrap_or(PAGE_BYTES);
        let read = source.read(&mut page[..wanted])?;
        if read == 0 {
            return Err(invalid("it ends too soon"));
        }
        target.write_all(&page[..read])?;
        left -= read as u64;
        work.copied_bytes = work.copied_bytes.saturating_add(read as u64);
        work.largest_read = work.largest_read.max(read);
    }
    Ok(())
}

fn spool<T: Serialize>(out: &mut (impl Write + ?Sized), value: &T) -> Result<(), HistoryError> {
    serde_json::to_writer(&mut *out, value).map_err(json_error)?;
    out.write_all(b"\n")?;
    Ok(())
}

/// Spool records contain only a filename and metadata, never native transcript contents.
fn next_spooled<T: serde::de::DeserializeOwned>(
    input: &mut impl BufRead,
) -> Result<Option<T>, HistoryError> {
    let mut line = String::new();
    let read = input.take((PAGE_BYTES + 1) as u64).read_line(&mut line)?;
    if read == 0 {
        return Ok(None);
    }
    if read > PAGE_BYTES || !line.ends_with('\n') {
        return Err(invalid("an archive metadata entry is too large"));
    }
    serde_json::from_str(&line).map(Some).map_err(json_error)
}

fn add_source(path: &Path, relative: String, index: &mut impl Write) -> Result<(), HistoryError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(invalid("a native archive source is not a regular file"));
    }
    spool(
        index,
        &SourceEntry {
            entry: Entry {
                path: relative,
                size: metadata.len(),
            },
            modified: metadata.modified().ok(),
        },
    )
}

/// Breadth-first traversal uses a disk queue, with one directory entry held at a time. Neither fanout
/// nor depth creates an in-memory collection, and symlinks cannot make traversal repeat or leave home.
fn index_sources(
    transcript: &Path,
    id: &str,
    into: &Path,
    cancel: &AtomicBool,
) -> Result<File, HistoryError> {
    let root = transcript
        .parent()
        .ok_or_else(|| invalid("the transcript has no directory"))?;
    let mut index = BufWriter::with_capacity(PAGE_BYTES, tempfile::tempfile_in(into)?);
    add_source(transcript, format!("{id}.jsonl"), &mut index)?;
    let directories = tempfile::NamedTempFile::new_in(into)?;
    let mut pending = BufWriter::with_capacity(PAGE_BYTES, directories.reopen()?);
    let mut visited = BufReader::with_capacity(PAGE_BYTES, directories.reopen()?);
    spool(&mut pending, &id)?;
    pending.flush()?;
    while let Some(relative) = next_spooled::<String>(&mut visited)? {
        check_cancel(cancel)?;
        let directory = root.join(&relative);
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound && relative == id => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            check_cancel(cancel)?;
            let entry = entry?;
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .map_err(|error| invalid(error.to_string()))?
                .to_str()
                .ok_or_else(|| invalid("a native filename is not UTF-8"))?
                .replace('\\', "/");
            let kind = entry.file_type()?;
            if kind.is_dir() {
                spool(&mut pending, &relative)?;
            } else if kind.is_file() {
                add_source(&path, relative, &mut index)?;
            } else {
                return Err(invalid(
                    "a native archive source is not a regular file or directory",
                ));
            }
        }
        pending.flush()?;
    }
    index.flush()?;
    let mut index = index.into_inner().map_err(|error| error.into_error())?;
    index.rewind()?;
    Ok(index)
}

fn export_files(
    home: &Path,
    id: &str,
    into: &Path,
    cancel: &AtomicBool,
) -> Result<(SessionArchive, Work), HistoryError> {
    check_cancel(cancel)?;
    let transcript =
        find_transcript(home, id).ok_or_else(|| HistoryError::NoTranscript(id.to_owned()))?;
    let version = version_in_reader(CancellableRead {
        input: File::open(&transcript)?,
        cancel,
    });
    check_cancel(cancel)?;
    std::fs::create_dir_all(into)?;
    let index = index_sources(&transcript, id, into, cancel)?;
    let mut index = BufReader::with_capacity(PAGE_BYTES, index);
    let target = into.join(format!("{id}.orch-claude-code"));
    let mut temporary = tempfile::NamedTempFile::new_in(into)?;
    let mut out = BufWriter::with_capacity(PAGE_BYTES, temporary.as_file_mut());
    write!(out, "{ARCHIVE_MAGIC}\n{{\"session_id\":")?;
    serde_json::to_writer(&mut out, id).map_err(json_error)?;
    out.write_all(b",\"harness_version\":")?;
    serde_json::to_writer(&mut out, &version).map_err(json_error)?;
    out.write_all(b",\"files\":[")?;
    let mut work = Work::default();
    while let Some(source) = next_spooled::<SourceEntry>(&mut index)? {
        check_cancel(cancel)?;
        if work.entries > 0 {
            out.write_all(b",")?;
        }
        serde_json::to_writer(&mut out, &source.entry).map_err(json_error)?;
        work.entries += 1;
    }
    out.write_all(b"]}\n")?;
    index.rewind()?;
    let root = transcript
        .parent()
        .ok_or_else(|| invalid("the transcript has no directory"))?;
    while let Some(source) = next_spooled::<SourceEntry>(&mut index)? {
        check_cancel(cancel)?;
        let mut input = File::open(root.join(&source.entry.path))?;
        let unchanged = |metadata: &std::fs::Metadata| {
            metadata.len() == source.entry.size && metadata.modified().ok() == source.modified
        };
        if !unchanged(&input.metadata()?) {
            return Err(invalid("a source file changed during export"));
        }
        copy_file(&mut input, &mut out, source.entry.size, cancel, &mut work)?;
        if !unchanged(&input.metadata()?) {
            return Err(invalid("a source file changed during export"));
        }
    }
    out.flush()?;
    drop(out);
    temporary.persist(&target).map_err(|error| error.error)?;
    let size = std::fs::metadata(&target)?.len();
    Ok((
        SessionArchive {
            format: TRANSCRIPT_FORMAT.to_owned(),
            path: target.to_string_lossy().into_owned(),
            size: i64::try_from(size).unwrap_or(i64::MAX),
        },
        work,
    ))
}

/// Ends at the metadata newline, leaving the binary payload untouched. A single malicious JSON token
/// cannot grow the parser's scratch buffer; the files array itself has no length limit.
struct MetadataLine<'a, R> {
    input: &'a mut R,
    ended: bool,
    in_string: bool,
    escaped: bool,
    token: usize,
    peak: usize,
}

impl<R: BufRead> Read for MetadataLine<'_, R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.ended || out.is_empty() {
            return Ok(0);
        }
        let available = self.input.fill_buf()?;
        let mut used = 0;
        let mut written = 0;
        for &byte in available.iter().take(out.len()) {
            used += 1;
            if byte == b'\n' {
                self.ended = true;
                break;
            }
            if self.in_string {
                self.token += 1;
                if self.escaped {
                    self.escaped = false;
                } else if byte == b'\\' {
                    self.escaped = true;
                } else if byte == b'"' {
                    self.in_string = false;
                }
            } else if byte == b'"' {
                self.in_string = true;
                self.token = 1;
            } else if byte.is_ascii_whitespace() || b"{}[]:,".contains(&byte) {
                self.token = 0;
            } else {
                self.token += 1;
            }
            self.peak = self.peak.max(self.token);
            if self.token > PAGE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "an archive metadata token is too large",
                ));
            }
            out[written] = byte;
            written += 1;
        }
        self.input.consume(used);
        Ok(written)
    }
}

struct HeaderSeed<'a> {
    index: &'a mut dyn Write,
    cancel: &'a AtomicBool,
    work: &'a mut Work,
}
struct EntriesSeed<'a> {
    index: &'a mut dyn Write,
    cancel: &'a AtomicBool,
    work: &'a mut Work,
}

impl<'de> DeserializeSeed<'de> for EntriesSeed<'_> {
    type Value = u64;
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<u64, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for EntriesSeed<'_> {
    type Value = u64;
    fn expecting(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("native archive files")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<u64, A::Error> {
        let mut bytes = 0_u64;
        while let Some(entry) = sequence.next_element::<Entry>()? {
            check_cancel(self.cancel).map_err(serde::de::Error::custom)?;
            bytes = bytes
                .checked_add(entry.size)
                .ok_or_else(|| serde::de::Error::custom("the archive size overflows"))?;
            spool(&mut *self.index, &entry).map_err(serde::de::Error::custom)?;
            self.work.entries += 1;
        }
        Ok(bytes)
    }
}

impl<'de> DeserializeSeed<'de> for HeaderSeed<'_> {
    type Value = Header;
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Header, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for HeaderSeed<'_> {
    type Value = Header;
    fn expecting(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("a native archive header")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Header, A::Error> {
        let mut id = None;
        let mut version: Option<Option<String>> = None;
        let mut bytes = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "session_id" if id.is_none() => {
                    id = Some(map.next_value()?);
                }
                "harness_version" if version.is_none() => {
                    version = Some(map.next_value()?);
                }
                "files" if bytes.is_none() => {
                    bytes = Some(map.next_value_seed(EntriesSeed {
                        index: &mut *self.index,
                        cancel: self.cancel,
                        work: &mut *self.work,
                    })?);
                }
                "session_id" | "harness_version" | "files" => {
                    return Err(serde::de::Error::custom("a header field is repeated"));
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(Header {
            session_id: id.ok_or_else(|| serde::de::Error::missing_field("session_id"))?,
            harness_version: version.flatten(),
            payload_bytes: bytes.ok_or_else(|| serde::de::Error::missing_field("files"))?,
        })
    }
}

fn read_header(
    input: &mut impl BufRead,
    index: &mut File,
    cancel: &AtomicBool,
    work: &mut Work,
) -> Result<Header, HistoryError> {
    let mut line = MetadataLine {
        input,
        ended: false,
        in_string: false,
        escaped: false,
        token: 0,
        peak: 0,
    };
    let mut buffered = BufWriter::with_capacity(PAGE_BYTES, &mut *index);
    let mut json = serde_json::Deserializer::from_reader(&mut line);
    let header = HeaderSeed {
        index: &mut buffered,
        cancel,
        work: &mut *work,
    }
    .deserialize(&mut json)
    .map_err(json_error)?;
    json.end().map_err(json_error)?;
    drop(json);
    buffered.flush()?;
    drop(buffered);
    if !line.ended {
        return Err(invalid("the header ends too soon"));
    }
    work.largest_metadata_token = work.largest_metadata_token.max(line.peak);
    index.rewind()?;
    Ok(header)
}

fn stays_below(path: &str, id: &str) -> bool {
    (path == format!("{id}.jsonl") || path.starts_with(&format!("{id}/")))
        && Path::new(path)
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}

fn import_files(
    home: &Path,
    managed: bool,
    path: &Path,
    target_version: &str,
    cancel: &AtomicBool,
) -> Result<(String, Work), HistoryError> {
    check_cancel(cancel)?;
    let mut input = BufReader::with_capacity(PAGE_BYTES, File::open(path)?);
    if next_spooled_magic(&mut input)? != ARCHIVE_MAGIC {
        return Err(invalid("its first line is wrong"));
    }
    let mut index = tempfile::tempfile_in(home)?;
    let mut work = Work::default();
    let header = read_header(&mut input, &mut index, cancel, &mut work)?;
    if let Some(version) = &header.harness_version {
        let approved = crate::descriptor::support_of(version)
            == orch_agent_contract::descriptor::HarnessSupport::Supported
            || version == PORTABLE_ARCHIVE_CANDIDATE;
        if !approved {
            return Err(HistoryError::OtherVersion(version.clone()));
        }
        if version != target_version {
            return Err(HistoryError::VersionMismatch {
                archive: version.clone(),
                target: target_version.to_owned(),
            });
        }
    }
    if uuid_like(&header.session_id).is_none() {
        return Err(invalid("the session id is not an id"));
    }
    if input
        .get_ref()
        .metadata()?
        .len()
        .saturating_sub(input.stream_position()?)
        < header.payload_bytes
    {
        return Err(invalid("it ends too soon"));
    }
    let mut entries = BufReader::with_capacity(PAGE_BYTES, index);
    // All paths and the complete captured payload length are checked before creating destination files.
    while let Some(entry) = next_spooled::<Entry>(&mut entries)? {
        check_cancel(cancel)?;
        if !stays_below(&entry.path, &header.session_id) {
            return Err(invalid(format!("{:?} is outside the session", entry.path)));
        }
    }
    entries.rewind()?;
    let project = if managed {
        MANAGED_PROJECT_DIR.to_owned()
    } else {
        format!("-{MANAGED_PROJECT_DIR}-imported")
    };
    let directory = super::projects_directory(home).join(project);
    while let Some(entry) = next_spooled::<Entry>(&mut entries)? {
        check_cancel(cancel)?;
        let target = directory.join(&entry.path);
        let parent = target
            .parent()
            .ok_or_else(|| invalid("a file has no parent"))?;
        std::fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        copy_file(
            &mut input,
            temporary.as_file_mut(),
            entry.size,
            cancel,
            &mut work,
        )?;
        temporary.persist(&target).map_err(|error| error.error)?;
    }
    Ok((header.session_id, work))
}

fn next_spooled_magic(input: &mut impl BufRead) -> Result<String, HistoryError> {
    let mut line = String::new();
    input.take((PAGE_BYTES + 1) as u64).read_line(&mut line)?;
    if line.len() > PAGE_BYTES || !line.ends_with('\n') {
        return Err(invalid("its first line is incomplete or too large"));
    }
    line.pop();
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "11111111-1111-4111-8111-111111111111";

    #[test]
    fn scale_a_million_native_items_round_trip_with_page_sized_reads() {
        let home = tempfile::tempdir().unwrap();
        let project = home.path().join("projects/orch");
        std::fs::create_dir_all(&project).unwrap();
        let source = project.join(format!("{ID}.jsonl"));
        let mut out = BufWriter::new(File::create(&source).unwrap());
        writeln!(out, "{{\"type\":\"session_start\"}}").unwrap();
        for item in 0..1_000_000 {
            writeln!(out, "{{\"type\":\"assistant\",\"item\":{item},\"text\":\"native bytes remain unchanged across a move\"}}").unwrap();
        }
        writeln!(out, "{{\"version\":\"2.1.312\"}}").unwrap();
        out.flush().unwrap();
        let into = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let (archive, exported) = export_files(home.path(), ID, into.path(), &cancel).unwrap();
        let target = tempfile::tempdir().unwrap();
        let (id, imported) = import_files(
            target.path(),
            true,
            Path::new(&archive.path),
            "2.1.312",
            &cancel,
        )
        .unwrap();
        assert_eq!(id, ID);
        let size = std::fs::metadata(&source).unwrap().len();
        assert!(size > 80 * 1024 * 1024);
        assert_eq!(exported.copied_bytes, size);
        assert_eq!(imported.copied_bytes, size);
        assert_eq!(exported.entries, 1);
        assert_eq!(imported.entries, 1);
        assert_eq!(exported.largest_read, PAGE_BYTES);
        assert!(imported.largest_read <= PAGE_BYTES);
        let mut original = File::open(&source).unwrap();
        let mut moved =
            File::open(super::super::find_transcript(target.path(), ID).unwrap()).unwrap();
        let mut left = [0; PAGE_BYTES];
        let mut right = [0; PAGE_BYTES];
        loop {
            let count = original.read(&mut left).unwrap();
            if count == 0 {
                break;
            }
            moved.read_exact(&mut right[..count]).unwrap();
            assert_eq!(left[..count], right[..count]);
        }
        assert_eq!(moved.read(&mut right).unwrap(), 0);
    }

    #[test]
    fn scale_a_million_file_entries_are_spooled_instead_of_collected() {
        let mut input = tempfile::tempfile().unwrap();
        write!(
            input,
            "{{\"session_id\":\"{ID}\",\"harness_version\":\"2.1.312\",\"files\":["
        )
        .unwrap();
        for file in 0..1_000_000 {
            if file > 0 {
                input.write_all(b",").unwrap();
            }
            write!(
                input,
                "{{\"path\":\"{ID}/subagents/agent-{file}.jsonl\",\"size\":0}}"
            )
            .unwrap();
        }
        input.write_all(b"]}\nPAYLOAD").unwrap();
        input.rewind().unwrap();
        let mut input = BufReader::with_capacity(PAGE_BYTES, input);
        let mut index = tempfile::tempfile().unwrap();
        let mut work = Work::default();
        let header =
            read_header(&mut input, &mut index, &AtomicBool::new(false), &mut work).unwrap();
        assert_eq!(header.payload_bytes, 0);
        assert_eq!(work.entries, 1_000_000);
        assert!(work.largest_metadata_token <= PAGE_BYTES);
        let mut index = BufReader::with_capacity(PAGE_BYTES, index);
        for file in 0..1_000_000 {
            let entry = next_spooled::<Entry>(&mut index).unwrap().unwrap();
            assert_eq!(entry.path, format!("{ID}/subagents/agent-{file}.jsonl"));
        }
        assert!(next_spooled::<Entry>(&mut index).unwrap().is_none());
        let mut payload = [0; 7];
        input.read_exact(&mut payload).unwrap();
        assert_eq!(&payload, b"PAYLOAD");
    }

    #[test]
    fn cancelled_copy_reads_no_more_native_bytes() {
        let mut source = io::Cursor::new([1; PAGE_BYTES]);
        let mut work = Work::default();
        let result = copy_file(
            &mut source,
            &mut io::sink(),
            PAGE_BYTES as u64,
            &AtomicBool::new(true),
            &mut work,
        );
        assert!(
            matches!(result, Err(HistoryError::Io(ref error)) if error.kind() == io::ErrorKind::Interrupted)
        );
        assert_eq!(source.position(), 0);
        assert_eq!(work.copied_bytes, 0);
    }

    #[test]
    fn scale_a_hundred_thousand_subagent_files_use_a_disk_directory_queue() {
        let home = tempfile::tempdir().unwrap();
        let project = home.path().join("projects/orch");
        std::fs::create_dir_all(&project).unwrap();
        let transcript = project.join(format!("{ID}.jsonl"));
        std::fs::write(&transcript, "{\"version\":\"2.1.312\"}\n").unwrap();
        for group in 0..1_000 {
            let directory = project.join(ID).join(format!("group-{group}"));
            std::fs::create_dir_all(&directory).unwrap();
            for file in 0..100 {
                File::create(directory.join(format!("agent-{file}.jsonl"))).unwrap();
            }
        }
        let into = tempfile::tempdir().unwrap();
        let index = index_sources(&transcript, ID, into.path(), &AtomicBool::new(false)).unwrap();
        let mut input = BufReader::with_capacity(PAGE_BYTES, index);
        let first = next_spooled::<SourceEntry>(&mut input).unwrap().unwrap();
        assert_eq!(first.entry.path, format!("{ID}.jsonl"));
        let mut entries = 1;
        while let Some(entry) = next_spooled::<SourceEntry>(&mut input).unwrap() {
            assert!(stays_below(&entry.entry.path, ID));
            assert_eq!(entry.entry.size, 0);
            entries += 1;
        }
        assert_eq!(entries, 100_001);
    }

    #[test]
    fn oversized_metadata_and_truncated_payloads_are_refused_before_writing() {
        let target = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let path = source.path().join("archive");
        let cancel = AtomicBool::new(false);
        let bad_path = "x".repeat(PAGE_BYTES + 1);
        std::fs::write(&path, format!("{ARCHIVE_MAGIC}\n{{\"session_id\":\"{ID}\",\"files\":[{{\"path\":\"{bad_path}\",\"size\":0}}]}}\n")).unwrap();
        assert!(import_files(target.path(), true, &path, "2.1.312", &cancel).is_err());
        assert!(!target.path().join("projects").exists());
        std::fs::write(&path, format!("{ARCHIVE_MAGIC}\n{{\"session_id\":\"{ID}\",\"files\":[{{\"path\":\"{ID}.jsonl\",\"size\":1000000000}}]}}\nx")).unwrap();
        assert!(import_files(target.path(), true, &path, "2.1.312", &cancel).is_err());
        assert!(!target.path().join("projects").exists());
    }
}
