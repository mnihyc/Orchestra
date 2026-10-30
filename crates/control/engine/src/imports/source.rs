//! What an import reads: the files a person uploaded, as a set of named entries that are read one at a time and never unpacked onto a disk.
//!
//! An archive is a zip kept in the store of content, read through a window of it (a file's index is read once, and an entry
//! by the bytes it lies in); loose files are the uploads themselves, each under the name the person gave it. Both answer the same
//! few questions: which entries there are and how large each is, the bytes of one (bounded), and a copy of one in a scratch file
//! with its two hashes, which is what a file that goes into the store is made from. Paths are looked up by name and never joined
//! onto a directory, so that a name with `..` in it can name nothing outside the archive; the names are checked all the same.

use async_trait::async_trait;
use bytes::Bytes;
use orch_blob_contract::range::ByteRange;
use orch_blob_contract::store::{BlobStore, read_all};
use orch_types::Hash;
use sha2::{Digest as _, Sha256};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

/// How much of an archive is fetched from the store at one time: a quarter of a mebibyte, the size the store reads in.
const WINDOW_BYTES: u64 = 256 * 1024;

/// How many bytes at the end of a zip are searched for its end-of-index record: the record and the longest comment.
const ZIP_END_SEARCH_BYTES: u64 = 22 + 65_535;

/// The signature of the end-of-index record of a zip, of the 64-bit locator before it and of the 64-bit record it points at.
const ZIP_END: [u8; 4] = *b"PK\x05\x06";
const ZIP64_LOCATOR: [u8; 4] = *b"PK\x06\x07";
const ZIP64_END: [u8; 4] = *b"PK\x06\x06";

/// The first bytes of a zip that holds files, and of one that holds none.
const ZIP_MAGIC: [&[u8]; 2] = [b"PK\x03\x04", b"PK\x05\x06"];

/// Why a source could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    /// The upload is not there any more, or is not the person's.
    #[error("{0} is not there any more")]
    Missing(String),
    /// The archive is damaged, or is not one this can read.
    #[error("this cannot be read: {0}")]
    Unreadable(String),
    /// A name that no entry can have: absolute, with `..`, with a back slash.
    #[error("{0} is not a path inside the source")]
    BadPath(String),
    /// The entry is not in the source.
    #[error("{0} is not in the source")]
    NotFound(String),
    /// The entry is larger than may be read.
    #[error("{entry} is larger than the {limit} bytes that may be read")]
    TooLarge {
        /// The entry.
        entry: String,
        /// The most bytes that may be read.
        limit: u64,
    },
    /// The entry is encrypted, a link or a directory.
    #[error("{0} is not a plain file")]
    NotAFile(String),
    /// What an entry holds is not what the manifest says of it.
    #[error("{0} does not match its hash")]
    HashMismatch(String),
    /// The source holds more entries than may be looked into.
    #[error(
        "the source holds {found} entries, and one that holds more than {limit} is not looked into"
    )]
    TooManyEntries {
        /// How many it claims.
        found: u64,
        /// The most.
        limit: u64,
    },
    /// The store or the reading itself failed.
    #[error("{0}")]
    Failed(String),
}

fn failed(error: impl std::fmt::Display) -> SourceError {
    SourceError::Failed(error.to_string())
}

fn unreadable(error: impl std::fmt::Display) -> SourceError {
    SourceError::Unreadable(error.to_string())
}

/// A name as an entry of a source may have it: relative, with `/` between its parts, none of which is empty, `.`, `..` or
/// holds a control character or a back slash. A leading `./` is taken off, and so is a trailing `/` (a directory).
pub fn clean_path(name: &str) -> Result<String, SourceError> {
    let trimmed = name.strip_prefix("./").unwrap_or(name);
    let trimmed = trimmed.strip_suffix('/').unwrap_or(trimmed);
    if trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.contains('\\')
        || trimmed.chars().any(char::is_control)
        || trimmed
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(SourceError::BadPath(name.to_owned()));
    }
    Ok(trimmed.to_owned())
}

/// One entry of a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryFact {
    /// Its path.
    pub name: String,
    /// Its size, once expanded.
    pub size: u64,
    /// Whether it is a directory.
    pub dir: bool,
}

/// A copy of an entry in a scratch file, with what its content hashes to.
#[derive(Debug)]
pub struct Spooled {
    /// The file.
    pub path: PathBuf,
    /// Its size in bytes.
    pub size: u64,
    /// The hash the store keeps it under.
    pub blob: Hash,
    /// The SHA-256 in lower case hexadecimal, which a package's manifest lists.
    pub sha256: String,
}

/// What an import reads, entry by entry.
#[async_trait]
pub trait Entries: Send + Sync {
    /// Every entry, in the order of the source.
    fn facts(&self) -> &[EntryFact];

    /// The bytes of the entry `name`, at most `limit` of them.
    async fn read(&self, name: &str, limit: u64) -> Result<Vec<u8>, SourceError>;

    /// A copy of the entry `name` in a new file under `dir`, at most `limit` bytes of it.
    async fn spool(&self, name: &str, limit: u64, dir: &Path) -> Result<Spooled, SourceError>;

    /// The SHA-256 of the entry `name`, in lower case hexadecimal, read without keeping it; at most `limit` bytes of it.
    async fn sha256(&self, name: &str, limit: u64) -> Result<String, SourceError>;
}

// ---------------------------------------------------------------- a blob as a file

/// A blob of the store read as a file: bytes by position, fetched from the store in windows. It is for a thread that
/// may block.
struct Windowed {
    store: Arc<dyn BlobStore>,
    hash: Hash,
    size: u64,
    runtime: tokio::runtime::Handle,
    position: u64,
    window_start: u64,
    window: Bytes,
}

impl Windowed {
    fn open(store: &Arc<dyn BlobStore>, hash: Hash, size: u64) -> Self {
        Self {
            store: Arc::clone(store),
            hash,
            size,
            runtime: tokio::runtime::Handle::current(),
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
        if bytes.len() as u64 != end - start + 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "the blob range does not have its stated length",
            ));
        }
        self.window_start = start;
        self.window = bytes;
        Ok(())
    }
}

impl Read for Windowed {
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

impl Seek for Windowed {
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

/// How many entries the index of a zip claims, read from its end-of-index record without reading the index: the 64-bit
/// record when the ordinary one says so.
fn zip_entries<R: Read + Seek>(source: &mut R, size: u64) -> Result<u64, SourceError> {
    let tail = size.min(ZIP_END_SEARCH_BYTES);
    source
        .seek(SeekFrom::Start(size - tail))
        .map_err(unreadable)?;
    let mut bytes = vec![0_u8; usize::try_from(tail).unwrap_or(0)];
    source.read_exact(&mut bytes).map_err(unreadable)?;
    let at = bytes
        .windows(ZIP_END.len())
        .rposition(|window| window == ZIP_END)
        .ok_or_else(|| SourceError::Unreadable("the zip has no end of index".to_owned()))?;
    let record = bytes
        .get(at..at + 22)
        .ok_or_else(|| SourceError::Unreadable("the end of index is cut short".to_owned()))?;
    let total = u64::from(u16::from_le_bytes([record[10], record[11]]));
    if total != u64::from(u16::MAX) {
        return Ok(total);
    }
    let locator = at
        .checked_sub(20)
        .and_then(|from| bytes.get(from..at))
        .filter(|locator| locator.starts_with(&ZIP64_LOCATOR))
        .ok_or_else(|| SourceError::Unreadable("the zip has no 64-bit end of index".to_owned()))?;
    let mut offset = [0_u8; 8];
    offset.copy_from_slice(&locator[8..16]);
    source
        .seek(SeekFrom::Start(u64::from_le_bytes(offset)))
        .map_err(unreadable)?;
    let mut end = [0_u8; 56];
    source.read_exact(&mut end).map_err(unreadable)?;
    if !end.starts_with(&ZIP64_END) {
        return Err(SourceError::Unreadable(
            "the 64-bit end of index is not where the zip says".to_owned(),
        ));
    }
    let mut count = [0_u8; 8];
    count.copy_from_slice(&end[32..40]);
    Ok(u64::from_le_bytes(count))
}

/// Whether the first bytes of a file are those of a zip.
#[must_use]
pub fn looks_like_zip(head: &[u8]) -> bool {
    ZIP_MAGIC.iter().any(|magic| head.starts_with(magic))
}

// ---------------------------------------------------------------- a zip

/// A zip in the store of content.
pub struct ZipEntries {
    archive: Arc<Mutex<zip::ZipArchive<Windowed>>>,
    facts: Vec<EntryFact>,
}

impl ZipEntries {
    /// Opens the zip kept under `hash`, which is `size` bytes. One whose index claims more than `most` entries is not read.
    pub async fn open(
        store: Arc<dyn BlobStore>,
        hash: Hash,
        size: u64,
        most: u64,
    ) -> Result<Self, SourceError> {
        tokio::task::spawn_blocking(move || {
            let mut source = Windowed::open(&store, hash, size);
            let claimed = zip_entries(&mut source, size)?;
            if claimed > most {
                return Err(SourceError::TooManyEntries {
                    found: claimed,
                    limit: most,
                });
            }
            source.seek(SeekFrom::Start(0)).map_err(unreadable)?;
            let mut archive = zip::ZipArchive::new(source).map_err(unreadable)?;
            let facts = zip_facts(&mut archive)?;
            Ok(Self {
                archive: Arc::new(Mutex::new(archive)),
                facts,
            })
        })
        .await
        .map_err(failed)?
    }
}

/// The entries of a zip as its index lists them, in its order; a link or an encrypted entry is listed as a file of no size, which is
/// refused when it is read.
fn zip_facts<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> Result<Vec<EntryFact>, SourceError> {
    let mut facts = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(unreadable)?;
        let raw = entry.name().map_err(unreadable)?.to_string();
        let dir = entry.is_dir();
        if entry.is_symlink() || entry.encrypted() {
            facts.push(EntryFact {
                name: raw,
                size: 0,
                dir: false,
            });
            continue;
        }
        facts.push(EntryFact {
            name: raw,
            size: entry.size(),
            dir,
        });
    }
    Ok(facts)
}

/// The bytes of the entry `name` of a zip, at most `limit` of them.
fn zip_read<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Result<Vec<u8>, SourceError> {
    let mut entry = archive
        .by_name(name)
        .map_err(|_| SourceError::NotFound(name.to_owned()))?;
    if entry.is_dir() || entry.is_symlink() || entry.encrypted() {
        return Err(SourceError::NotAFile(name.to_owned()));
    }
    if entry.size() > limit {
        return Err(SourceError::TooLarge {
            entry: name.to_owned(),
            limit,
        });
    }
    read_bounded(&mut entry, name, limit)
}

/// A copy of the entry `name` of a zip in a new file under `dir`.
fn zip_spool<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    limit: u64,
    dir: &Path,
) -> Result<Spooled, SourceError> {
    let mut entry = archive
        .by_name(name)
        .map_err(|_| SourceError::NotFound(name.to_owned()))?;
    if entry.is_dir() || entry.is_symlink() || entry.encrypted() {
        return Err(SourceError::NotAFile(name.to_owned()));
    }
    spool_reader(&mut entry, name, limit, dir)
}

/// The SHA-256 of the entry `name` of a zip, at most `limit` bytes of it.
fn zip_sha256<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Result<String, SourceError> {
    let mut entry = archive
        .by_name(name)
        .map_err(|_| SourceError::NotFound(name.to_owned()))?;
    if entry.is_dir() || entry.is_symlink() || entry.encrypted() {
        return Err(SourceError::NotAFile(name.to_owned()));
    }
    hash_reader(&mut entry, name, limit)
}

/// Reads at most `limit` bytes of a reader, refusing one that holds more.
fn read_bounded(reader: &mut dyn Read, name: &str, limit: u64) -> Result<Vec<u8>, SourceError> {
    let mut bytes = Vec::new();
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(failed)?;
    if bytes.len() as u64 > limit {
        return Err(SourceError::TooLarge {
            entry: name.to_owned(),
            limit,
        });
    }
    Ok(bytes)
}

/// The SHA-256 of what a reader holds, at most `limit` bytes of it.
fn hash_reader(reader: &mut dyn Read, name: &str, limit: u64) -> Result<String, SourceError> {
    let mut sha = Sha256::new();
    let mut buffer = vec![0_u8; 256 * 1024];
    let mut size: u64 = 0;
    loop {
        let read = reader.read(&mut buffer).map_err(failed)?;
        if read == 0 {
            return Ok(hex::encode(sha.finalize()));
        }
        size = size.saturating_add(read as u64);
        if size > limit {
            return Err(SourceError::TooLarge {
                entry: name.to_owned(),
                limit,
            });
        }
        sha.update(&buffer[..read]);
    }
}

/// Copies a reader into a new file under `dir`, hashing what passes, at most `limit` bytes of it.
fn spool_reader(
    reader: &mut dyn Read,
    name: &str,
    limit: u64,
    dir: &Path,
) -> Result<Spooled, SourceError> {
    std::fs::create_dir_all(dir).map_err(failed)?;
    let path = dir.join(format!("import-{}.part", orch_types::new_id().simple()));
    let mut file = std::fs::File::create(&path).map_err(failed)?;
    let mut blake = Hash::hasher();
    let mut sha = Sha256::new();
    let mut buffer = vec![0_u8; 256 * 1024];
    let mut size: u64 = 0;
    let outcome = (|| -> Result<(), SourceError> {
        loop {
            let read = reader.read(&mut buffer).map_err(failed)?;
            if read == 0 {
                return Ok(());
            }
            size = size.saturating_add(read as u64);
            if size > limit {
                return Err(SourceError::TooLarge {
                    entry: name.to_owned(),
                    limit,
                });
            }
            blake.update(&buffer[..read]);
            sha.update(&buffer[..read]);
            file.write_all(&buffer[..read]).map_err(failed)?;
        }
    })();
    if let Err(error) = outcome {
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    Ok(Spooled {
        path,
        size,
        blob: Hash::finish(&blake),
        sha256: hex::encode(sha.finalize()),
    })
}

#[async_trait]
impl Entries for ZipEntries {
    fn facts(&self) -> &[EntryFact] {
        &self.facts
    }

    async fn read(&self, name: &str, limit: u64) -> Result<Vec<u8>, SourceError> {
        let (archive, name) = (Arc::clone(&self.archive), name.to_owned());
        tokio::task::spawn_blocking(move || {
            let mut archive = archive.lock().unwrap_or_else(PoisonError::into_inner);
            zip_read(&mut archive, &name, limit)
        })
        .await
        .map_err(failed)?
    }

    async fn spool(&self, name: &str, limit: u64, dir: &Path) -> Result<Spooled, SourceError> {
        let (archive, name, dir) = (Arc::clone(&self.archive), name.to_owned(), dir.to_owned());
        tokio::task::spawn_blocking(move || {
            let mut archive = archive.lock().unwrap_or_else(PoisonError::into_inner);
            zip_spool(&mut archive, &name, limit, &dir)
        })
        .await
        .map_err(failed)?
    }

    async fn sha256(&self, name: &str, limit: u64) -> Result<String, SourceError> {
        let (archive, name) = (Arc::clone(&self.archive), name.to_owned());
        tokio::task::spawn_blocking(move || {
            let mut archive = archive.lock().unwrap_or_else(PoisonError::into_inner);
            zip_sha256(&mut archive, &name, limit)
        })
        .await
        .map_err(failed)?
    }
}

// ---------------------------------------------------------------- a zip that was inside another, and archives that make one source

/// A zip in a scratch file of its own: one that was inside another zip (a Notion export is a zip of zips), taken out to be read by
/// position. The file is removed when this is dropped.
pub struct ZipFileEntries {
    archive: Arc<Mutex<zip::ZipArchive<std::fs::File>>>,
    path: PathBuf,
    facts: Vec<EntryFact>,
}

impl ZipFileEntries {
    /// Opens the zip in the file `path`, which is removed with it (or now, where the zip is not one this can read): one whose index
    /// claims more than `most` entries is not read.
    pub async fn open(path: PathBuf, most: u64) -> Result<Self, SourceError> {
        let at = path.clone();
        let made = tokio::task::spawn_blocking(move || {
            let mut file = std::fs::File::open(&at).map_err(failed)?;
            let size = file.metadata().map_err(failed)?.len();
            let claimed = zip_entries(&mut file, size)?;
            if claimed > most {
                return Err(SourceError::TooManyEntries {
                    found: claimed,
                    limit: most,
                });
            }
            file.seek(SeekFrom::Start(0)).map_err(unreadable)?;
            let mut archive = zip::ZipArchive::new(file).map_err(unreadable)?;
            let facts = zip_facts(&mut archive)?;
            Ok((archive, facts))
        })
        .await
        .map_err(failed)
        .and_then(|made| made);
        match made {
            Ok((archive, facts)) => Ok(Self {
                archive: Arc::new(Mutex::new(archive)),
                path,
                facts,
            }),
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                Err(error)
            }
        }
    }
}

impl Drop for ZipFileEntries {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[async_trait]
impl Entries for ZipFileEntries {
    fn facts(&self) -> &[EntryFact] {
        &self.facts
    }

    async fn read(&self, name: &str, limit: u64) -> Result<Vec<u8>, SourceError> {
        let (archive, name) = (Arc::clone(&self.archive), name.to_owned());
        tokio::task::spawn_blocking(move || {
            let mut archive = archive.lock().unwrap_or_else(PoisonError::into_inner);
            zip_read(&mut archive, &name, limit)
        })
        .await
        .map_err(failed)?
    }

    async fn spool(&self, name: &str, limit: u64, dir: &Path) -> Result<Spooled, SourceError> {
        let (archive, name, dir) = (Arc::clone(&self.archive), name.to_owned(), dir.to_owned());
        tokio::task::spawn_blocking(move || {
            let mut archive = archive.lock().unwrap_or_else(PoisonError::into_inner);
            zip_spool(&mut archive, &name, limit, &dir)
        })
        .await
        .map_err(failed)?
    }

    async fn sha256(&self, name: &str, limit: u64) -> Result<String, SourceError> {
        let (archive, name) = (Arc::clone(&self.archive), name.to_owned());
        tokio::task::spawn_blocking(move || {
            let mut archive = archive.lock().unwrap_or_else(PoisonError::into_inner);
            zip_sha256(&mut archive, &name, limit)
        })
        .await
        .map_err(failed)?
    }
}

/// Archives that are read as one source: the zips a Notion export comes in, inside one another or chosen together. An entry is read from
/// the part that holds it; a name that is in two parts is the later part's, and a folder is listed once.
pub struct PartsEntries {
    parts: Vec<Arc<dyn Entries>>,
    owner: HashMap<String, usize>,
    facts: Vec<EntryFact>,
}

impl PartsEntries {
    /// The parts, in the order they were given.
    #[must_use]
    pub fn new(parts: Vec<Arc<dyn Entries>>) -> Self {
        let mut facts: Vec<EntryFact> = Vec::new();
        let mut owner: HashMap<String, usize> = HashMap::new();
        let mut at: HashMap<String, usize> = HashMap::new();
        for (index, part) in parts.iter().enumerate() {
            for fact in part.facts() {
                match at.get(&fact.name) {
                    Some(each) => {
                        if !fact.dir {
                            facts[*each].size = fact.size;
                            owner.insert(fact.name.clone(), index);
                        }
                    }
                    None => {
                        at.insert(fact.name.clone(), facts.len());
                        facts.push(fact.clone());
                        if !fact.dir {
                            owner.insert(fact.name.clone(), index);
                        }
                    }
                }
            }
        }
        Self {
            parts,
            owner,
            facts,
        }
    }

    fn part_of(&self, name: &str) -> Result<&Arc<dyn Entries>, SourceError> {
        self.owner
            .get(name)
            .and_then(|index| self.parts.get(*index))
            .ok_or_else(|| SourceError::NotFound(name.to_owned()))
    }
}

#[async_trait]
impl Entries for PartsEntries {
    fn facts(&self) -> &[EntryFact] {
        &self.facts
    }

    async fn read(&self, name: &str, limit: u64) -> Result<Vec<u8>, SourceError> {
        self.part_of(name)?.read(name, limit).await
    }

    async fn spool(&self, name: &str, limit: u64, dir: &Path) -> Result<Spooled, SourceError> {
        self.part_of(name)?.spool(name, limit, dir).await
    }

    async fn sha256(&self, name: &str, limit: u64) -> Result<String, SourceError> {
        self.part_of(name)?.sha256(name, limit).await
    }
}

// ---------------------------------------------------------------- a tar

/// How a tar is packed, which the first bytes of the file say and its name does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Packing {
    /// Not packed.
    Plain,
    /// With gzip.
    Gzip,
    /// With bzip2.
    Bzip2,
    /// With xz.
    Xz,
    /// With zstd.
    Zstd,
}

impl Packing {
    /// What the first bytes of a file are packed in; a tar that is not packed begins with a name.
    #[must_use]
    pub fn of(head: &[u8]) -> Self {
        if head.starts_with(&[0x1f, 0x8b]) {
            Self::Gzip
        } else if head.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
            Self::Zstd
        } else if head.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00]) {
            Self::Xz
        } else if head.starts_with(b"BZh") {
            Self::Bzip2
        } else {
            Self::Plain
        }
    }
}

/// How much memory the decoder of an xz may use: what the strongest setting of the tool itself needs, with room. An xz that asks for
/// more is refused, whatever its size, so that a small file cannot ask for gigabytes.
const XZ_MEMORY_BYTES: u64 = 256 * 1024 * 1024;

/// A tar, packed or not, expanded once into a scratch file and read from there by position. A tar has no index to look an entry up
/// by, so the copy is what is looked into once and read from afterwards; it is bounded by the bytes the place lets an import hold,
/// and removed when the import lets go of it.
pub struct TarEntries {
    file: Arc<Mutex<std::fs::File>>,
    path: PathBuf,
    facts: Vec<EntryFact>,
    /// Where the data of each file lies in the copy, and how long it is; none for a link, which is listed and read as no file.
    places: HashMap<String, Option<(u64, u64)>>,
}

/// What looking into a tar found.
struct Scanned {
    facts: Vec<EntryFact>,
    places: HashMap<String, Option<(u64, u64)>>,
}

/// Copies `source` (unpacked, where it is packed) into the file `copy`, at most `most_bytes` of it.
fn expand<R: Read>(
    source: R,
    packing: Packing,
    copy: &Path,
    most_bytes: u64,
) -> Result<(), SourceError> {
    let mut reader: Box<dyn Read + '_> = match packing {
        Packing::Plain => Box::new(source),
        Packing::Gzip => Box::new(flate2::read::MultiGzDecoder::new(source)),
        Packing::Bzip2 => Box::new(bzip2::read::MultiBzDecoder::new(source)),
        Packing::Xz => {
            let stream = liblzma::stream::Stream::new_stream_decoder(
                XZ_MEMORY_BYTES,
                liblzma::stream::CONCATENATED,
            )
            .map_err(failed)?;
            Box::new(liblzma::read::XzDecoder::new_stream(source, stream))
        }
        Packing::Zstd => Box::new(zstd::stream::read::Decoder::new(source).map_err(failed)?),
    };
    let mut file = std::fs::File::create(copy).map_err(failed)?;
    let copied = std::io::copy(
        &mut (&mut reader).take(most_bytes.saturating_add(1)),
        &mut file,
    )
    .map_err(unreadable)?;
    if copied > most_bytes {
        return Err(SourceError::TooLarge {
            entry: "the archive".to_owned(),
            limit: most_bytes,
        });
    }
    Ok(())
}

/// Looks into the tar in `copy`: its folders and its plain files with where the data of each lies, and its links, which are listed
/// as files that cannot be read (as a zip's are); a device, a header of the format and a file laid out in pieces are not among them.
/// An entry that is there twice is the later one.
fn scan(copy: &Path, most_entries: u64) -> Result<Scanned, SourceError> {
    let mut archive = tar::Archive::new(std::fs::File::open(copy).map_err(failed)?);
    let mut facts: Vec<EntryFact> = Vec::new();
    let mut places: HashMap<String, Option<(u64, u64)>> = HashMap::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for entry in archive.entries_with_seek().map_err(unreadable)? {
        let entry = entry.map_err(unreadable)?;
        let counted = u64::try_from(facts.len()).unwrap_or(u64::MAX);
        if counted >= most_entries {
            return Err(SourceError::TooManyEntries {
                found: counted.saturating_add(1),
                limit: most_entries,
            });
        }
        let kind = entry.header().entry_type();
        let raw = entry
            .path()
            .map_err(unreadable)?
            .to_string_lossy()
            .into_owned();
        if kind.is_dir() {
            facts.push(EntryFact {
                name: raw,
                size: 0,
                dir: true,
            });
            continue;
        }
        let link = kind.is_symlink() || kind.is_hard_link();
        if !link && !matches!(kind, tar::EntryType::Regular | tar::EntryType::Continuous) {
            continue;
        }
        let Ok(name) = clean_path(&raw) else {
            // A name that no entry can have: kept as it is, for the importer to count and leave out.
            facts.push(EntryFact {
                name: raw,
                size: 0,
                dir: false,
            });
            continue;
        };
        let (size, place) = if link {
            (0, None)
        } else {
            (
                entry.size(),
                Some((entry.raw_file_position(), entry.size())),
            )
        };
        places.insert(name.clone(), place);
        match at.get(&name) {
            Some(each) => facts[*each].size = size,
            None => {
                at.insert(name.clone(), facts.len());
                facts.push(EntryFact {
                    name,
                    size,
                    dir: false,
                });
            }
        }
    }
    Ok(Scanned { facts, places })
}

impl TarEntries {
    /// Opens the tar kept under `hash`, which is `size` bytes and packed as `packing` says: expanded into a file under `scratch`,
    /// refused where it expands to more than `most_bytes` or holds more than `most_entries` entries.
    pub async fn open(
        store: Arc<dyn BlobStore>,
        hash: Hash,
        size: u64,
        packing: Packing,
        most_entries: u64,
        most_bytes: u64,
        scratch: PathBuf,
    ) -> Result<Self, SourceError> {
        tokio::task::spawn_blocking(move || {
            let source = Windowed::open(&store, hash, size);
            Self::from_reader(source, packing, most_entries, most_bytes, &scratch)
        })
        .await
        .map_err(failed)?
    }

    /// As [`TarEntries::open`], for a tar read from `source`.
    fn from_reader<R: Read>(
        source: R,
        packing: Packing,
        most_entries: u64,
        most_bytes: u64,
        scratch: &Path,
    ) -> Result<Self, SourceError> {
        std::fs::create_dir_all(scratch).map_err(failed)?;
        let path = scratch.join(format!("import-{}.tar", orch_types::new_id().simple()));
        let made = expand(source, packing, &path, most_bytes).and_then(|()| {
            let scanned = scan(&path, most_entries)?;
            let file = std::fs::File::open(&path).map_err(failed)?;
            Ok((scanned, file))
        });
        match made {
            Ok((scanned, file)) => Ok(Self {
                file: Arc::new(Mutex::new(file)),
                path,
                facts: scanned.facts,
                places: scanned.places,
            }),
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                Err(error)
            }
        }
    }

    /// Where the data of the file `name` lies: its first byte and its length.
    fn place(&self, name: &str) -> Result<(u64, u64), SourceError> {
        match self.places.get(name) {
            Some(Some(place)) => Ok(*place),
            Some(None) => Err(SourceError::NotAFile(name.to_owned())),
            None => Err(SourceError::NotFound(name.to_owned())),
        }
    }
}

impl Drop for TarEntries {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[async_trait]
impl Entries for TarEntries {
    fn facts(&self) -> &[EntryFact] {
        &self.facts
    }

    async fn read(&self, name: &str, limit: u64) -> Result<Vec<u8>, SourceError> {
        let (at, length) = self.place(name)?;
        if length > limit {
            return Err(SourceError::TooLarge {
                entry: name.to_owned(),
                limit,
            });
        }
        let (file, name) = (Arc::clone(&self.file), name.to_owned());
        tokio::task::spawn_blocking(move || {
            let mut file = file.lock().unwrap_or_else(PoisonError::into_inner);
            file.seek(SeekFrom::Start(at)).map_err(failed)?;
            read_bounded(&mut (&mut *file).take(length), &name, limit)
        })
        .await
        .map_err(failed)?
    }

    async fn spool(&self, name: &str, limit: u64, dir: &Path) -> Result<Spooled, SourceError> {
        let (at, length) = self.place(name)?;
        let (file, name, dir) = (Arc::clone(&self.file), name.to_owned(), dir.to_owned());
        tokio::task::spawn_blocking(move || {
            let mut file = file.lock().unwrap_or_else(PoisonError::into_inner);
            file.seek(SeekFrom::Start(at)).map_err(failed)?;
            spool_reader(&mut (&mut *file).take(length), &name, limit, &dir)
        })
        .await
        .map_err(failed)?
    }

    async fn sha256(&self, name: &str, limit: u64) -> Result<String, SourceError> {
        let (at, length) = self.place(name)?;
        let (file, name) = (Arc::clone(&self.file), name.to_owned());
        tokio::task::spawn_blocking(move || {
            let mut file = file.lock().unwrap_or_else(PoisonError::into_inner);
            file.seek(SeekFrom::Start(at)).map_err(failed)?;
            hash_reader(&mut (&mut *file).take(length), &name, limit)
        })
        .await
        .map_err(failed)?
    }
}

// ---------------------------------------------------------------- loose files

/// Files that were uploaded one by one, each under the name the person gave it.
pub struct LooseEntries {
    store: Arc<dyn BlobStore>,
    facts: Vec<EntryFact>,
    blobs: HashMap<String, (Hash, u64)>,
}

impl LooseEntries {
    /// The uploads `files`, by name; the store says how large each is.
    pub async fn open(
        store: Arc<dyn BlobStore>,
        files: &[(String, Hash)],
    ) -> Result<Self, SourceError> {
        let mut facts = Vec::with_capacity(files.len());
        let mut blobs = HashMap::new();
        for (name, hash) in files {
            let name = clean_path(name)?;
            let size = store
                .size(hash)
                .await
                .map_err(failed)?
                .ok_or_else(|| SourceError::Missing(name.clone()))?;
            let size = u64::try_from(size).map_err(failed)?;
            if blobs.insert(name.clone(), (*hash, size)).is_some() {
                return Err(SourceError::Unreadable(format!(
                    "{name} is named twice in the source"
                )));
            }
            facts.push(EntryFact {
                name,
                size,
                dir: false,
            });
        }
        Ok(Self {
            store,
            facts,
            blobs,
        })
    }

    /// A file's immutable size is already known: refuse it before fetching bytes, then read the same windows as a zip.
    fn window(&self, name: &str, limit: u64) -> Result<Windowed, SourceError> {
        let &(hash, size) = self
            .blobs
            .get(name)
            .ok_or_else(|| SourceError::NotFound(name.to_owned()))?;
        if size > limit {
            return Err(SourceError::TooLarge {
                entry: name.to_owned(),
                limit,
            });
        }
        Ok(Windowed::open(&self.store, hash, size))
    }
}

#[async_trait]
impl Entries for LooseEntries {
    fn facts(&self) -> &[EntryFact] {
        &self.facts
    }

    async fn read(&self, name: &str, limit: u64) -> Result<Vec<u8>, SourceError> {
        let (hash, _) = self
            .blobs
            .get(name)
            .ok_or_else(|| SourceError::NotFound(name.to_owned()))?;
        let read = self.store.get(hash, None).await.map_err(failed)?;
        if u64::try_from(read.meta.size).unwrap_or(u64::MAX) > limit {
            return Err(SourceError::TooLarge {
                entry: name.to_owned(),
                limit,
            });
        }
        Ok(read_all(read.body).await.map_err(failed)?.to_vec())
    }

    async fn spool(&self, name: &str, limit: u64, dir: &Path) -> Result<Spooled, SourceError> {
        let mut reader = self.window(name, limit)?;
        let dir = dir.to_owned();
        let name = name.to_owned();
        tokio::task::spawn_blocking(move || spool_reader(&mut reader, &name, limit, &dir))
            .await
            .map_err(failed)?
    }

    async fn sha256(&self, name: &str, limit: u64) -> Result<String, SourceError> {
        let mut reader = self.window(name, limit)?;
        let name = name.to_owned();
        tokio::task::spawn_blocking(move || hash_reader(&mut reader, &name, limit))
            .await
            .map_err(failed)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_blob_contract::range::BlobMeta;
    use orch_blob_contract::store::{BlobError, BlobRead, ByteStream, Stored, stream_of};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A large logical upload generated one window at a time; a whole-file request fails the test's operation.
    struct RangedOnlyStore {
        hash: Hash,
        size: u64,
        short: bool,
        reads: AtomicU64,
        largest: AtomicU64,
    }

    impl RangedOnlyStore {
        fn new(size: u64, short: bool) -> (Arc<Self>, String) {
            let mut blake = Hash::hasher();
            let mut sha = Sha256::new();
            let buffer = vec![b'x'; WINDOW_BYTES as usize];
            let mut remaining = size;
            while remaining > 0 {
                let count = remaining.min(WINDOW_BYTES) as usize;
                blake.update(&buffer[..count]);
                sha.update(&buffer[..count]);
                remaining -= count as u64;
            }
            (
                Arc::new(Self {
                    hash: Hash::finish(&blake),
                    size,
                    short,
                    reads: AtomicU64::new(0),
                    largest: AtomicU64::new(0),
                }),
                hex::encode(sha.finalize()),
            )
        }
    }

    #[async_trait]
    impl BlobStore for RangedOnlyStore {
        async fn put(
            &self,
            _hash: &Hash,
            _size: Option<i64>,
            _content: ByteStream,
        ) -> Result<Stored, BlobError> {
            Err(BlobError::Io("the source is read only".to_owned()))
        }

        async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
            if hash != &self.hash {
                return Err(BlobError::NotFound(*hash));
            }
            let size = self.size as i64;
            let range = range
                .ok_or_else(|| BlobError::Io("a whole-file read is forbidden".to_owned()))?
                .resolve(size)
                .map_err(|_| BlobError::RangeNotSatisfiable { size })?;
            let len = range.len as u64;
            if len > WINDOW_BYTES {
                return Err(BlobError::Io("a read exceeds one window".to_owned()));
            }
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.largest.fetch_max(len, Ordering::SeqCst);
            let actual = if self.short { len - 1 } else { len };
            Ok(BlobRead {
                meta: BlobMeta { hash: *hash, size },
                range,
                body: stream_of(vec![Bytes::from(vec![b'x'; actual as usize])]),
            })
        }

        async fn exists(&self, hash: &Hash) -> Result<bool, BlobError> {
            Ok(hash == &self.hash)
        }

        async fn size(&self, hash: &Hash) -> Result<Option<i64>, BlobError> {
            Ok((hash == &self.hash).then_some(self.size as i64))
        }

        async fn delete(&self, _hash: &Hash) -> Result<bool, BlobError> {
            Err(BlobError::Io("the source is read only".to_owned()))
        }
    }

    #[tokio::test]
    async fn loose_files_spool_and_hash_in_windows_even_past_64_mib() {
        // Hundreds of full windows and a partial tail expose both unbounded reads and a lost final chunk.
        let size = WINDOW_BYTES * 256 + 13;
        let (store, sha) = RangedOnlyStore::new(size, false);
        let source = LooseEntries::open(store.clone(), &[("large.bin".to_owned(), store.hash)])
            .await
            .unwrap();
        let scratch = tempfile::tempdir().unwrap();
        assert!(matches!(
            source.spool("large.bin", size - 1, scratch.path()).await,
            Err(SourceError::TooLarge { .. })
        ));
        assert!(matches!(
            source.sha256("large.bin", size - 1).await,
            Err(SourceError::TooLarge { .. })
        ));
        assert_eq!(
            store.reads.load(Ordering::SeqCst),
            0,
            "limits precede content reads"
        );
        let spooled = source
            .spool("large.bin", size, scratch.path())
            .await
            .unwrap();
        assert_eq!(spooled.size, size);
        assert_eq!(std::fs::metadata(&spooled.path).unwrap().len(), size);
        assert_eq!(spooled.blob, store.hash);
        assert_eq!(spooled.sha256, sha);
        assert_eq!(source.sha256("large.bin", size).await.unwrap(), sha);
        assert_eq!(store.largest.load(Ordering::SeqCst), WINDOW_BYTES);
        assert_eq!(
            store.reads.load(Ordering::SeqCst),
            2 * size.div_ceil(WINDOW_BYTES)
        );
    }

    #[tokio::test]
    async fn an_empty_loose_file_needs_no_range_and_a_truncated_one_leaves_no_partial_spool() {
        let (empty, sha) = RangedOnlyStore::new(0, false);
        let source = LooseEntries::open(empty.clone(), &[("empty.bin".to_owned(), empty.hash)])
            .await
            .unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let spooled = source.spool("empty.bin", 0, scratch.path()).await.unwrap();
        assert_eq!(spooled.size, 0);
        assert_eq!(spooled.blob, empty.hash);
        assert_eq!(spooled.sha256, sha);
        assert_eq!(source.sha256("empty.bin", 0).await.unwrap(), sha);
        assert_eq!(empty.reads.load(Ordering::SeqCst), 0);

        let (short, _) = RangedOnlyStore::new(WINDOW_BYTES + 1, true);
        let source = LooseEntries::open(short.clone(), &[("short.bin".to_owned(), short.hash)])
            .await
            .unwrap();
        let scratch = tempfile::tempdir().unwrap();
        assert!(matches!(
            source.spool("short.bin", short.size, scratch.path()).await,
            Err(SourceError::Failed(_))
        ));
        assert!(matches!(
            source.sha256("short.bin", short.size).await,
            Err(SourceError::Failed(_))
        ));
        assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_name_that_leaves_the_source_or_hides_a_part_is_no_path_inside_it() {
        assert_eq!(clean_path("a/b.md").unwrap(), "a/b.md");
        assert_eq!(clean_path("./a/b.md").unwrap(), "a/b.md");
        assert_eq!(clean_path("a/dir/").unwrap(), "a/dir");
        for bad in [
            "",
            "/etc/passwd",
            "../up.md",
            "a/../b",
            "a//b",
            "a\\b",
            "a/./b",
            "a/\u{7}b",
        ] {
            assert!(clean_path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_first_bytes_of_a_zip_tell_it_from_text() {
        assert!(looks_like_zip(b"PK\x03\x04rest"));
        assert!(looks_like_zip(b"PK\x05\x06"));
        assert!(!looks_like_zip(b"# Notes"));
        assert_eq!(Packing::of(&[0x1f, 0x8b, 0x08]), Packing::Gzip);
        assert_eq!(Packing::of(b"BZh91AY"), Packing::Bzip2);
        assert_eq!(
            Packing::of(&[0xfd, b'7', b'z', b'X', b'Z', 0x00, 0x00]),
            Packing::Xz
        );
        assert_eq!(Packing::of(&[0x28, 0xb5, 0x2f, 0xfd, 0x04]), Packing::Zstd);
        assert_eq!(Packing::of(b"# Notes"), Packing::Plain);
        assert_eq!(Packing::of(b""), Packing::Plain);
    }

    /// `a_tar()` packed as `packing` says.
    fn packed(packing: Packing) -> Vec<u8> {
        let tar = a_tar();
        match packing {
            Packing::Plain => tar,
            Packing::Gzip => {
                let mut out =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
                out.write_all(&tar).unwrap();
                out.finish().unwrap()
            }
            Packing::Bzip2 => {
                let mut out = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
                out.write_all(&tar).unwrap();
                out.finish().unwrap()
            }
            Packing::Xz => {
                let mut out = liblzma::write::XzEncoder::new(Vec::new(), 1);
                out.write_all(&tar).unwrap();
                out.finish().unwrap()
            }
            Packing::Zstd => zstd::encode_all(tar.as_slice(), 1).unwrap(),
        }
    }

    /// A tar of a folder, two notes, a picture, a note called twice and a link.
    fn a_tar() -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut folder = tar::Header::new_gnu();
        folder.set_entry_type(tar::EntryType::Directory);
        folder.set_size(0);
        folder.set_mode(0o755);
        builder
            .append_data(&mut folder, "notes/", std::io::empty())
            .unwrap();
        for (path, body) in [
            ("notes/a.md", &b"# A\n"[..]),
            ("notes/b.md", &b"# B, first\n"[..]),
            ("notes/pic.png", &[0x89, b'P', b'N', b'G'][..]),
            ("notes/b.md", &b"# B, last\n"[..]),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            builder.append_data(&mut header, path, body).unwrap();
        }
        let mut link = tar::Header::new_gnu();
        link.set_entry_type(tar::EntryType::Symlink);
        link.set_size(0);
        builder
            .append_link(&mut link, "notes/link.md", "a.md")
            .unwrap();
        builder.into_inner().unwrap()
    }

    fn scratch() -> PathBuf {
        std::env::temp_dir().join(format!("orch-tar-{}", orch_types::new_id().simple()))
    }

    #[tokio::test]
    async fn a_tar_is_looked_into_once_and_its_files_are_read_by_position_the_later_of_a_name_that_is_twice_and_no_link()
     {
        let dir = scratch();
        let tar = TarEntries::from_reader(
            std::io::Cursor::new(a_tar()),
            Packing::Plain,
            100,
            1 << 20,
            &dir,
        )
        .unwrap();
        let names: Vec<(&str, bool)> = tar
            .facts()
            .iter()
            .map(|fact| (fact.name.as_str(), fact.dir))
            .collect();
        assert_eq!(
            names,
            vec![
                ("notes/", true),
                ("notes/a.md", false),
                ("notes/b.md", false),
                ("notes/pic.png", false),
                ("notes/link.md", false)
            ],
            "a link is listed, and a name that is there twice is one"
        );
        assert_eq!(tar.read("notes/a.md", 1_000).await.unwrap(), b"# A\n");
        assert_eq!(
            tar.read("notes/b.md", 1_000).await.unwrap(),
            b"# B, last\n",
            "the later one"
        );
        assert!(matches!(
            tar.read("notes/b.md", 3).await,
            Err(SourceError::TooLarge { .. })
        ));
        assert!(matches!(
            tar.read("notes/link.md", 1_000).await,
            Err(SourceError::NotAFile(_))
        ));
        assert!(matches!(
            tar.read("notes/none.md", 1_000).await,
            Err(SourceError::NotFound(_))
        ));
        let spooled = tar.spool("notes/pic.png", 1_000, &dir).await.unwrap();
        assert_eq!(
            std::fs::read(&spooled.path).unwrap(),
            [0x89, b'P', b'N', b'G']
        );
        assert_eq!(spooled.size, 4);
        assert_eq!(
            tar.sha256("notes/a.md", 1_000).await.unwrap(),
            hex::encode(Sha256::digest(b"# A\n"))
        );
        let copy = tar.path.clone();
        assert!(copy.exists());
        drop(tar);
        assert!(!copy.exists(), "the copy goes with the entries");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_tar_packed_in_any_way_is_read_the_same_expanded_within_the_bytes_the_place_allows_and_one_with_too_many_entries_is_not_looked_into()
     {
        let dir = scratch();
        for packing in [
            Packing::Plain,
            Packing::Gzip,
            Packing::Bzip2,
            Packing::Xz,
            Packing::Zstd,
        ] {
            let bytes = packed(packing);
            assert_eq!(
                Packing::of(&bytes),
                packing,
                "the first bytes say how it is packed"
            );
            let tar = TarEntries::from_reader(
                std::io::Cursor::new(bytes.clone()),
                packing,
                100,
                1 << 20,
                &dir,
            )
            .unwrap();
            assert_eq!(
                tar.read("notes/a.md", 1_000).await.unwrap(),
                b"# A\n",
                "{packing:?}"
            );
            assert_eq!(
                tar.read("notes/b.md", 1_000).await.unwrap(),
                b"# B, last\n",
                "{packing:?}"
            );
            drop(tar);
            // Expanded, it is more than the bytes allowed: refused, and nothing is left behind.
            let refused = TarEntries::from_reader(
                std::io::Cursor::new(bytes.clone()),
                packing,
                100,
                1_000,
                &dir,
            );
            assert!(
                matches!(refused, Err(SourceError::TooLarge { .. })),
                "{packing:?}"
            );
            let few =
                TarEntries::from_reader(std::io::Cursor::new(bytes), packing, 2, 1 << 20, &dir);
            assert!(
                matches!(few, Err(SourceError::TooManyEntries { .. })),
                "{packing:?}"
            );
        }
        // Bytes that are not what the name says they are no tar.
        let broken = TarEntries::from_reader(
            std::io::Cursor::new(b"not a gzip at all".to_vec()),
            Packing::Gzip,
            100,
            1 << 20,
            &dir,
        );
        assert!(broken.is_err());
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert!(
            left.is_empty(),
            "a tar that is refused leaves no copy: {left:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A zip of the files `entries` name, in the file `path`.
    fn zip_file(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, bytes) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    #[tokio::test]
    async fn zips_read_as_one_source_list_the_entries_of_all_of_them_and_the_later_part_has_a_name_that_is_in_two_and_the_files_go_with_the_parts()
     {
        let dir = scratch();
        std::fs::create_dir_all(&dir).unwrap();
        let (one, two) = (dir.join("one.zip"), dir.join("two.zip"));
        zip_file(&one, &[("a.md", b"# A one\n"), ("both.md", b"first")]);
        zip_file(&two, &[("b.md", b"# B\n"), ("both.md", b"second")]);
        let parts: Vec<Arc<dyn Entries>> = vec![
            Arc::new(ZipFileEntries::open(one.clone(), 100).await.unwrap()),
            Arc::new(ZipFileEntries::open(two.clone(), 100).await.unwrap()),
        ];
        let all = PartsEntries::new(parts);
        let names: Vec<&str> = all.facts().iter().map(|fact| fact.name.as_str()).collect();
        assert_eq!(names, ["a.md", "both.md", "b.md"]);
        assert_eq!(all.facts()[1].size, 6, "the size of the later one");
        assert_eq!(all.read("a.md", 100).await.unwrap(), b"# A one\n");
        assert_eq!(all.read("both.md", 100).await.unwrap(), b"second");
        assert_eq!(
            all.sha256("b.md", 100).await.unwrap(),
            hex::encode(Sha256::digest(b"# B\n"))
        );
        assert!(matches!(
            all.read("none.md", 100).await,
            Err(SourceError::NotFound(_))
        ));
        drop(all);
        assert!(
            !one.exists() && !two.exists(),
            "the parts go with the source"
        );
        // A file that is no zip is refused, and removed.
        let bad = dir.join("bad.zip");
        std::fs::write(&bad, b"not a zip at all").unwrap();
        assert!(ZipFileEntries::open(bad.clone(), 100).await.is_err());
        assert!(!bad.exists(), "a file that is no zip is removed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
