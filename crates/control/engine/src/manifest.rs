//! Lists of a tree: the zstd-compressed JSON Lines blob a machine uploads when a tree changed, which the API keeps, reads and writes.
//!
//! A list is the tree at a moment. Its first line says what it is (`v`, `root`, `generated_at`,
//! `generation`, `case_sensitive`) and each other line is one entry, `{ p, t, s, m, x, h, l }`: the
//! path below the root of the list, the type (`f` file, `d` directory, `l` link, `o` other), the size,
//! the time of last change in nanoseconds, the permission bits, the hash of the content where the
//! machine knew it, and for a link where it points. The entries are sorted by path, byte by byte.
//! The API reads one to answer for a machine that is away, to pin what a snapshot took, and to learn
//! which files a command that writes a tree down may be given; it writes one to say which files are to
//! be written into a workspace.

use chrono::{DateTime, Utc};
use orch_types::{Hash, Time};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Write;

/// The version of the list that this reads.
pub const MANIFEST_VERSION: u32 = 1;

/// The most a list may expand to when it is read: a tree at the limit of what is indexed, written
/// out, is well under a gibibyte, and more than that is not a list of one.
pub const MANIFEST_MAX_EXPANDED_BYTES: u64 = 1024 * 1024 * 1024;

/// How hard a list is compressed when it is written: zstd's own default, which is quick and gets
/// most of the gain from text as repetitive as paths.
const COMPRESSION_LEVEL: i32 = 3;

/// The permission bits of a file the API writes into a list: what a file made by hand has.
pub const DEFAULT_FILE_MODE: u32 = 0o644;

/// Why a list of a tree could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// It is not a list this reads.
    #[error("that is not a list of a tree that this can read: {0}")]
    Unreadable(String),
}

/// What an entry of a list is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    /// A regular file.
    #[serde(rename = "f")]
    File,
    /// A directory.
    #[serde(rename = "d")]
    Dir,
    /// A symbolic link.
    #[serde(rename = "l")]
    Link,
    /// Anything else.
    #[serde(rename = "o")]
    Other,
}

/// The first line of a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    /// The version of the format.
    pub v: u32,
    /// The directory the paths are relative to, as the machine writes it.
    pub root: String,
    /// When it was made.
    pub generated_at: Time,
    /// The generation of the environment's unit when it was made.
    pub generation: i64,
    /// Whether names differ by case where it was made.
    pub case_sensitive: bool,
}

#[derive(Serialize, Deserialize)]
struct Line {
    p: String,
    t: EntryKind,
    s: i64,
    m: i64,
    x: u32,
    h: Option<Hash>,
    l: Option<String>,
}

/// One entry of a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Its path below the root of the list.
    pub path: String,
    /// What it is.
    pub kind: EntryKind,
    /// Its size in bytes; zero for anything but a file.
    pub size: i64,
    /// When its content changed, in nanoseconds since 1970.
    pub mtime_ns: i64,
    /// Its permission bits.
    pub mode: u32,
    /// The hash of its content, where the machine knew it.
    pub hash: Option<Hash>,
    /// For a link, where it points.
    pub link: Option<String>,
}

impl Entry {
    /// The last component of its path.
    #[must_use]
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// When it changed, as a moment.
    #[must_use]
    pub fn mtime(&self) -> DateTime<Utc> {
        DateTime::from_timestamp_nanos(self.mtime_ns)
    }
}

/// The list of a tree at a moment, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    header: Header,
    entries: Vec<Entry>,
}

impl Manifest {
    /// Reads a list: zstd-compressed JSON Lines, a header and then one entry on each line, sorted
    /// by path byte by byte. It has to be a version this reads and to expand to no more than a
    /// list may; the paths it names are checked as they are read, since a machine wrote it.
    pub fn parse(bytes: &[u8]) -> Result<Self, ManifestError> {
        use std::io::{BufRead, BufReader, Read};
        let bad = |what: String| ManifestError::Unreadable(what);
        let decoder = zstd::stream::read::Decoder::new(bytes)
            .map_err(|error| bad(error.to_string()))?
            .take(MANIFEST_MAX_EXPANDED_BYTES + 1);
        let mut lines = BufReader::new(decoder).lines();
        let first = lines
            .next()
            .ok_or_else(|| bad("it is empty".to_owned()))?
            .map_err(|error| bad(error.to_string()))?;
        let header: Header =
            serde_json::from_str(&first).map_err(|error| bad(error.to_string()))?;
        if header.v != MANIFEST_VERSION {
            return Err(bad(format!(
                "version {} is not version {MANIFEST_VERSION}",
                header.v
            )));
        }
        let mut expanded = first.len() as u64;
        let mut entries: Vec<Entry> = Vec::new();
        for line in lines {
            let line = line.map_err(|error| bad(error.to_string()))?;
            expanded += line.len() as u64 + 1;
            if expanded > MANIFEST_MAX_EXPANDED_BYTES {
                return Err(bad("it expands to more than a list may".to_owned()));
            }
            if line.is_empty() {
                continue;
            }
            let line: Line = serde_json::from_str(&line).map_err(|error| bad(error.to_string()))?;
            if line.p.starts_with('/')
                || line
                    .p
                    .split('/')
                    .any(|part| part == ".." || part.is_empty())
            {
                return Err(bad(format!("{:?} is not a path of a workspace", line.p)));
            }
            entries.push(Entry {
                path: line.p,
                kind: line.t,
                size: line.s,
                mtime_ns: line.m,
                mode: line.x,
                hash: line.h,
                link: line.l,
            });
        }
        // The machine sorts them; a list that is not sorted is sorted here, so that every lookup
        // can rely on it.
        if entries.windows(2).any(|pair| pair[0].path > pair[1].path) {
            entries.sort_by(|a, b| a.path.cmp(&b.path));
        }
        Ok(Self { header, entries })
    }

    /// What the first line says.
    #[must_use]
    pub const fn header(&self) -> &Header {
        &self.header
    }

    /// How many entries it has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether it names nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every entry, in the order of their paths.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The entry at `path`. The empty path is the root, which a list names by none.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&Entry> {
        self.entries
            .binary_search_by(|entry| entry.path.as_str().cmp(path))
            .ok()
            .and_then(|at| self.entries.get(at))
    }

    /// Whether anything lies at or below `path`: a directory a list implies without naming it
    /// still exists.
    #[must_use]
    pub fn has_below(&self, path: &str) -> bool {
        if path.is_empty() {
            return true;
        }
        let prefix = format!("{path}/");
        let start = self
            .entries
            .partition_point(|entry| entry.path.as_str() < prefix.as_str());
        self.entries
            .get(start)
            .is_some_and(|entry| entry.path.starts_with(&prefix))
    }

    /// What lies directly in the directory `directory` (the empty path is the root): its files and
    /// directories, with a directory that the list implies by what lies below it but does not name
    /// made up from that.
    #[must_use]
    pub fn children(&self, directory: &str) -> Vec<Entry> {
        let prefix = if directory.is_empty() {
            String::new()
        } else {
            format!("{directory}/")
        };
        let start = self
            .entries
            .partition_point(|entry| entry.path.as_str() < prefix.as_str());
        let mut found: Vec<Entry> = Vec::new();
        let mut named: HashSet<&str> = HashSet::new();
        let mut implied: Vec<(&str, &Entry)> = Vec::new();
        for entry in &self.entries[start..] {
            let Some(rest) = entry.path.strip_prefix(&prefix) else {
                break;
            };
            if rest.is_empty() {
                continue;
            }
            match rest.split_once('/') {
                None => {
                    if entry.kind == EntryKind::Dir {
                        named.insert(rest);
                    }
                    found.push(entry.clone());
                }
                Some((name, _)) => implied.push((name, entry)),
            }
        }
        let mut made: HashSet<&str> = HashSet::new();
        for (name, below) in implied {
            if named.contains(name) || !made.insert(name) {
                continue;
            }
            found.push(Entry {
                path: format!("{prefix}{name}"),
                kind: EntryKind::Dir,
                size: 0,
                mtime_ns: below.mtime_ns,
                mode: DEFAULT_DIRECTORY_MODE,
                hash: None,
                link: None,
            });
        }
        found
    }

    /// The entries whose path holds `needle`, a word in lower case, without regard to the case of
    /// the path: those whose own name holds it come first. At most `limit`.
    #[must_use]
    pub fn find(&self, needle: &str, limit: usize) -> Vec<&Entry> {
        let mut by_name: Vec<&Entry> = Vec::new();
        let mut by_path: Vec<&Entry> = Vec::new();
        for entry in &self.entries {
            if entry.path.to_lowercase().contains(needle) {
                if entry.name().to_lowercase().contains(needle) {
                    by_name.push(entry);
                } else {
                    by_path.push(entry);
                }
            }
            if by_name.len() >= limit {
                break;
            }
        }
        by_name.extend(by_path);
        by_name.truncate(limit);
        by_name
    }

    /// The hashes of the files whose content the list names.
    pub fn hashes(&self) -> impl Iterator<Item = Hash> + '_ {
        self.entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::File)
            .filter_map(|entry| entry.hash)
    }
}

/// The permission bits of a directory a list implies: what a directory made by hand has.
const DEFAULT_DIRECTORY_MODE: u32 = 0o755;

/// A list written: the compressed bytes and the hash that names them.
#[derive(Debug, Clone)]
pub struct Written {
    /// The compressed list.
    pub bytes: Vec<u8>,
    /// Its hash.
    pub hash: Hash,
    /// How many files it names.
    pub file_count: i64,
    /// How many bytes they have together.
    pub total_bytes: i64,
}

/// Writes a list of `entries`, which are put in the order of their paths, under `header`.
pub fn build(header: &Header, mut entries: Vec<Entry>) -> Result<Written, ManifestError> {
    let bad = |error: std::io::Error| ManifestError::Unreadable(error.to_string());
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut encoder =
        zstd::stream::write::Encoder::new(Vec::new(), COMPRESSION_LEVEL).map_err(bad)?;
    write_line(&mut encoder, header).map_err(bad)?;
    let (mut file_count, mut total_bytes) = (0_i64, 0_i64);
    for entry in &entries {
        if entry.kind == EntryKind::File {
            file_count += 1;
            total_bytes = total_bytes.saturating_add(entry.size);
        }
        let line = Line {
            p: entry.path.clone(),
            t: entry.kind,
            s: entry.size,
            m: entry.mtime_ns,
            x: entry.mode,
            h: entry.hash,
            l: entry.link.clone(),
        };
        write_line(&mut encoder, &line).map_err(bad)?;
    }
    let bytes = encoder.finish().map_err(bad)?;
    Ok(Written {
        hash: Hash::of(&bytes),
        bytes,
        file_count,
        total_bytes,
    })
}

fn write_line<T: Serialize>(out: &mut impl Write, value: &T) -> std::io::Result<()> {
    serde_json::to_writer(&mut *out, value).map_err(std::io::Error::other)?;
    out.write_all(b"\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(path: &str, kind: &str, size: i64, hash: Option<Hash>) -> serde_json::Value {
        serde_json::json!({
            "p": path, "t": kind, "s": size, "m": 1_791_484_919_451_000_000_i64, "x": 420,
            "h": hash, "l": null
        })
    }

    fn manifest_of(entries: &[serde_json::Value]) -> Vec<u8> {
        let header = serde_json::json!({
            "v": 1, "root": "/workspace", "generated_at": "2026-10-08T17:04:05.123Z",
            "generation": 3, "case_sensitive": true
        });
        let mut text = header.to_string();
        for entry in entries {
            text.push('\n');
            text.push_str(&entry.to_string());
        }
        zstd::encode_all(text.as_bytes(), 3).unwrap()
    }

    #[test]
    fn a_directory_is_listed_with_what_lies_directly_in_it_and_not_what_lies_deeper() {
        let hash = Hash::of(b"fn main() {}");
        let bytes = manifest_of(&[
            line("README.md", "f", 10, None),
            line("src", "d", 0, None),
            line("src-old", "d", 0, None),
            line("src-old/a.txt", "f", 1, None),
            line("src/lib.rs", "f", 5, None),
            line("src/main.rs", "f", 12, Some(hash)),
            line("src/util", "d", 0, None),
            line("src/util/mod.rs", "f", 7, None),
        ]);
        let tree = Manifest::parse(&bytes).unwrap();
        let root: Vec<String> = tree.children("").into_iter().map(|e| e.path).collect();
        assert_eq!(root, ["README.md", "src", "src-old"]);
        let src: Vec<String> = tree.children("src").into_iter().map(|e| e.path).collect();
        assert_eq!(src, ["src/lib.rs", "src/main.rs", "src/util"]);
        assert_eq!(tree.get("src/main.rs").unwrap().hash, Some(hash));
        assert!(tree.has_below("src/util"));
        assert!(!tree.has_below("srcx"));
        assert_eq!(tree.hashes().collect::<Vec<_>>(), [hash]);
    }

    #[test]
    fn a_directory_that_only_what_lies_below_it_names_is_made_up_once() {
        let bytes = manifest_of(&[
            line("a/b/one.txt", "f", 1, None),
            line("a/b/two.txt", "f", 2, None),
            line("a/c.txt", "f", 3, None),
        ]);
        let tree = Manifest::parse(&bytes).unwrap();
        let root = tree.children("");
        assert_eq!(root.len(), 1);
        assert_eq!(root[0].path, "a");
        assert_eq!(root[0].kind, EntryKind::Dir);
        let a: Vec<String> = tree.children("a").into_iter().map(|e| e.path).collect();
        assert_eq!(a, ["a/c.txt", "a/b"]);
    }

    #[test]
    fn the_names_of_files_are_searched_without_regard_to_case_and_a_name_beats_a_directory() {
        let bytes = manifest_of(&[
            line("docs/Readme.md", "f", 1, None),
            line("readme/notes.txt", "f", 1, None),
            line("src/main.rs", "f", 1, None),
        ]);
        let tree = Manifest::parse(&bytes).unwrap();
        let found: Vec<&str> = tree
            .find("readme", 10)
            .iter()
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(found, ["docs/Readme.md", "readme/notes.txt"]);
        assert_eq!(tree.find("readme", 1).len(), 1);
        assert!(tree.find("absent", 10).is_empty());
    }

    #[test]
    fn a_list_that_names_a_path_outside_a_workspace_or_is_another_version_is_not_read() {
        let outside = manifest_of(&[line("../etc/passwd", "f", 1, None)]);
        assert!(Manifest::parse(&outside).is_err());
        let absolute = manifest_of(&[line("/etc/passwd", "f", 1, None)]);
        assert!(Manifest::parse(&absolute).is_err());
        let future = zstd::encode_all(
            br#"{"v":2,"root":"/w","generated_at":"2026-10-08T17:04:05.123Z","generation":1,"case_sensitive":true}"#
                .as_slice(),
            3,
        )
        .unwrap();
        assert!(Manifest::parse(&future).is_err());
        assert!(Manifest::parse(b"not zstd at all").is_err());
    }
}
