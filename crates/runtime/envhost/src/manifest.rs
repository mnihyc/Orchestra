//! Sorted whole-tree manifests and publication.
//!
//! A manifest is the tree at a moment: a zstd-compressed JSON Lines blob whose first line says
//! what it is (`v`, `root`, `generated_at`, `generation`, `case_sensitive`) and whose other lines
//! are one entry each, `{ p, t, s, m, x, h, l }`: path, type (`f` file, `d` directory, `l` link,
//! `o` other), size, modification time in nanoseconds, permission bits, content hash where the
//! host knows it (else null) and link target. Entries are sorted by path, byte by byte.
//!
//! **Paths are written relative to the manifest's root**, which the first line gives as an
//! absolute path: the workspace root for a manifest of the whole tree, and the directory a
//! selection names when it names exactly one, so that a snapshot of a thread's directory can be
//! written under any other directory.
//!
//! A manifest is published when a turn has completed and the tree changed, every few minutes
//! while changes go on, when it is asked for, and before the environment goes offline when that
//! can be done; [`Publisher`] says when. The same module reads a manifest back, with a bound on
//! what it may expand to.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, Utc};
use orch_link_contract::commands::Selection;
use orch_types::{Clock, Hash, Time};
use serde::{Deserialize, Serialize};

use crate::error::{HostError, Result};
use crate::files::Files;
use crate::index::{HashCache, Index, IndexEntry};
use crate::paths::{Locate, PathStyle, RelPath, Workspace};
use crate::platform::Kind;
use crate::walk::{Flow, walk};

/// The version of the manifest format that is written.
pub const MANIFEST_VERSION: u32 = 1;

/// How hard a manifest is compressed: zstd's own default, which is quick and gets most of the gain
/// from text as repetitive as paths.
const COMPRESSION_LEVEL: i32 = 3;

/// The most a manifest may expand to when it is read. A tree at the entry limit written out is
/// well under a gibibyte; more than that is not a manifest of one.
pub const MANIFEST_MAX_EXPANDED_BYTES: u64 = 1024 * 1024 * 1024;

/// The first line of a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestHeader {
    /// The version of the format.
    pub v: u32,
    /// The directory the paths are relative to, as an absolute path on the machine that wrote it.
    pub root: String,
    /// When it was made.
    pub generated_at: Time,
    /// The generation of the environment's unit when it was made.
    pub generation: i64,
    /// Whether names differ by case where it was made.
    pub case_sensitive: bool,
}

/// What an entry is, in the one letter a manifest writes.
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

impl From<Kind> for EntryKind {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::File => Self::File,
            Kind::Dir => Self::Dir,
            Kind::Symlink => Self::Link,
            Kind::Other => Self::Other,
        }
    }
}

/// One entry of a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Its path below the manifest's root.
    pub p: String,
    /// What it is.
    pub t: EntryKind,
    /// Its size in bytes; zero for anything but a file.
    pub s: i64,
    /// When its content changed, in nanoseconds since 1970.
    pub m: i64,
    /// Its permission bits.
    pub x: u32,
    /// The hash of its content, where the host knows it.
    pub h: Option<Hash>,
    /// For a link, where it points.
    pub l: Option<String>,
}

/// A manifest, written.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// The compressed blob.
    pub bytes: Vec<u8>,
    /// Its hash, which names it in the store.
    pub hash: Hash,
    /// How many files it names.
    pub file_count: i64,
    /// How many bytes they have together.
    pub total_bytes: i64,
}

/// Writes the blob for `header` and `entries`, which are put in the order of their paths.
pub fn build(header: &ManifestHeader, mut entries: Vec<ManifestEntry>) -> Result<Manifest> {
    entries.sort_by(|a, b| a.p.cmp(&b.p));
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), COMPRESSION_LEVEL)
        .map_err(|error| HostError::io("starting a manifest", error))?;
    write_line(&mut encoder, header)?;
    let mut file_count = 0i64;
    let mut total_bytes = 0i64;
    for entry in &entries {
        if entry.t == EntryKind::File {
            file_count += 1;
            total_bytes = total_bytes.saturating_add(entry.s);
        }
        write_line(&mut encoder, entry)?;
    }
    let bytes = encoder
        .finish()
        .map_err(|error| HostError::io("finishing a manifest", error))?;
    let hash = Hash::of(&bytes);
    Ok(Manifest {
        bytes,
        hash,
        file_count,
        total_bytes,
    })
}

fn write_line<T: Serialize>(out: &mut impl Write, value: &T) -> Result<()> {
    serde_json::to_writer(&mut *out, value)
        .map_err(|error| HostError::io("writing a manifest", std::io::Error::other(error)))?;
    out.write_all(b"\n")
        .map_err(|error| HostError::io("writing a manifest", error))
}

/// A manifest, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// Its first line.
    pub header: ManifestHeader,
    /// Its entries.
    pub entries: Vec<ManifestEntry>,
}

/// Reads a manifest blob. It must be a manifest of a version this host reads, expand to no more
/// than [`MANIFEST_MAX_EXPANDED_BYTES`], and name only paths a workspace can hold.
pub fn parse(bytes: &[u8]) -> Result<Parsed> {
    let decoder = zstd::stream::read::Decoder::new(bytes)
        .map_err(|error| HostError::io("opening a manifest", error))?;
    let limited = decoder.take(MANIFEST_MAX_EXPANDED_BYTES + 1);
    let mut lines = BufReader::new(limited).lines();
    let bad = |what: &str| {
        HostError::Unsupported(format!("that is not a manifest this host can read: {what}"))
    };
    let first = lines
        .next()
        .ok_or_else(|| bad("it is empty"))?
        .map_err(|error| HostError::io("reading a manifest", error))?;
    let header: ManifestHeader =
        serde_json::from_str(&first).map_err(|error| bad(&error.to_string()))?;
    if header.v != MANIFEST_VERSION {
        return Err(bad(&format!(
            "version {} is not version {MANIFEST_VERSION}",
            header.v
        )));
    }
    let mut entries = Vec::new();
    let mut expanded = first.len() as u64;
    for line in lines {
        let line = line.map_err(|error| HostError::io("reading a manifest", error))?;
        expanded += line.len() as u64 + 1;
        if expanded > MANIFEST_MAX_EXPANDED_BYTES {
            return Err(bad("it expands to more than a manifest may"));
        }
        if line.is_empty() {
            continue;
        }
        let entry: ManifestEntry =
            serde_json::from_str(&line).map_err(|error| bad(&error.to_string()))?;
        // Every path is checked as it is read: a manifest is made by another machine.
        RelPath::parse(&entry.p, PathStyle::Posix)
            .map_err(|fault| bad(&format!("{:?}: {fault}", entry.p)))?;
        entries.push(entry);
    }
    Ok(Parsed { header, entries })
}

/// What a selection names, resolved against a workspace.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// The directory the manifest's paths are relative to: the one directory the selection names,
    /// or the workspace root.
    pub root: RelPath,
    /// The places to walk, as they really are, from the workspace root.
    pub includes: Vec<RelPath>,
    /// The places left out, from the workspace root.
    pub excludes: Vec<RelPath>,
    include_patterns: Vec<SelectionPattern>,
    exclude_patterns: Vec<SelectionPattern>,
    aliases: Vec<SelectionAlias>,
    included_matches: regex::RegexSet,
    excluded_matches: regex::RegexSet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectionAlias {
    asked: RelPath,
    real: RelPath,
}

#[derive(Debug, Clone)]
struct SelectionPattern {
    expression: String,
    prefix: RelPath,
    literal: bool,
}

impl SelectionPattern {
    fn parse(workspace: &Workspace, text: &str) -> Result<Self> {
        let converted = if workspace.style() == PathStyle::Windows {
            text.replace('\\', "/")
        } else {
            text.to_owned()
        };
        orch_link_contract::commands::validate_selection_pattern(&converted)
            .map_err(|reason| HostError::Unsupported(reason.to_owned()))?;
        let literal = !converted.contains(['*', '?']);
        let prefix = converted
            .split('/')
            .take_while(|part| !part.contains(['*', '?']))
            .collect::<Vec<_>>()
            .join("/");
        let prefix = workspace.parse(&prefix)?;
        let mut expression = String::from("^");
        let components: Vec<&str> = converted.split('/').collect();
        for (index, component) in components.iter().enumerate() {
            let last = index + 1 == components.len();
            if *component == "**" {
                expression.push_str(if last { ".*" } else { "(?:[^/]+/)*" });
                continue;
            }
            for character in component.chars() {
                match character {
                    '*' => expression.push_str("[^/]*"),
                    '?' => expression.push_str("[^/]"),
                    other => expression.push_str(&regex::escape(&other.to_string())),
                }
            }
            if !last {
                expression.push('/');
            }
        }
        if !converted.is_empty() {
            expression.push_str("(?:/.*)?");
        } else {
            expression.push_str(".*");
        }
        expression.push('$');
        Ok(Self {
            expression,
            prefix,
            literal,
        })
    }
}

fn pattern_set(workspace: &Workspace, patterns: &[SelectionPattern]) -> Result<regex::RegexSet> {
    // One compiled set applies the regex engine's existing aggregate memory bound to the entire
    // selection, instead of allowing each of many patterns that much memory independently.
    regex::RegexSetBuilder::new(patterns.iter().map(|pattern| pattern.expression.as_str()))
        .case_insensitive(!workspace.case_sensitive())
        .build()
        .map_err(|_| {
            HostError::Unsupported("the selection patterns exceed the matcher budget".to_owned())
        })
}

impl Resolved {
    /// Whether the selection is the whole tree, with nothing left out.
    #[must_use]
    pub fn is_whole_tree(&self) -> bool {
        self.excludes.is_empty()
            && self.exclude_patterns.is_empty()
            && self.includes.len() == 1
            && self.includes[0].is_root()
            && self
                .include_patterns
                .iter()
                .all(|pattern| pattern.literal && pattern.prefix.is_root())
    }

    pub(crate) fn excluded(&self, path: &RelPath) -> bool {
        self.excludes
            .iter()
            .any(|excluded| path.starts_with(excluded))
            || self.matches(&self.excluded_matches, path)
    }

    pub(crate) fn included(&self, path: &RelPath) -> bool {
        self.include_patterns.is_empty() || self.matches(&self.included_matches, path)
    }

    fn matches(&self, patterns: &regex::RegexSet, path: &RelPath) -> bool {
        if patterns.is_match(path.as_str()) {
            return true;
        }
        if patterns.is_empty() {
            return false;
        }
        // Only explicitly selected prefixes have aliases. A normal scan still sees links
        // as entries, and excluding an unselected link does not exclude its target's tree.
        self.aliases.iter().any(|alias| {
            let Some(suffix) = path.strip_prefix(&alias.real) else {
                return false;
            };
            let mut asked =
                String::with_capacity(alias.asked.as_str().len() + 1 + suffix.as_str().len());
            asked.push_str(alias.asked.as_str());
            if !suffix.is_root() {
                if !asked.is_empty() {
                    asked.push('/');
                }
                asked.push_str(suffix.as_str());
            }
            patterns.is_match(&asked)
        })
    }
}

/// Resolves `selection` in `workspace`: an empty one is the whole tree; a path that is not there
/// is not part of it.
pub fn resolve(workspace: &Workspace, selection: &Selection) -> Result<Resolved> {
    let mut includes: Vec<RelPath> = Vec::new();
    let mut include_patterns = Vec::new();
    let mut aliases = Vec::new();
    let mut single_is_dir = false;
    if selection.include.is_empty() {
        includes.push(RelPath::root());
        single_is_dir = true;
    }
    for text in &selection.include {
        let pattern = SelectionPattern::parse(workspace, text)?;
        let asked = pattern.prefix.clone();
        let Ok(located) = workspace.locate(&asked, Locate::THROUGH) else {
            include_patterns.push(pattern);
            continue;
        };
        let Some(meta) = located.meta else {
            include_patterns.push(pattern);
            continue;
        };
        single_is_dir = pattern.literal && meta.is_dir();
        if asked != located.real {
            let alias = SelectionAlias {
                asked,
                real: located.real.clone(),
            };
            if !aliases.contains(&alias) {
                aliases.push(alias);
            }
        }
        if !includes.contains(&located.real) {
            includes.push(located.real);
        }
        include_patterns.push(pattern);
    }
    let mut excludes = Vec::new();
    let mut exclude_patterns = Vec::new();
    for text in &selection.exclude {
        let pattern = SelectionPattern::parse(workspace, text)?;
        if pattern.literal {
            excludes.push(pattern.prefix.clone());
        }
        exclude_patterns.push(pattern);
    }
    let root = match includes.as_slice() {
        [only] if include_patterns.len() <= 1 && single_is_dir => only.clone(),
        [only]
            if include_patterns.len() <= 1
                && include_patterns.iter().all(|pattern| pattern.literal) =>
        {
            only.parent().unwrap_or_else(RelPath::root)
        }
        _ => RelPath::root(),
    };
    // A selection with overlapping prefixes still walks each directory once.
    let original = includes.clone();
    includes.retain(|path| {
        !original
            .iter()
            .any(|parent| path != parent && path.starts_with(parent))
    });
    let included_matches = pattern_set(workspace, &include_patterns)?;
    let excluded_matches = pattern_set(workspace, &exclude_patterns)?;
    Ok(Resolved {
        root,
        includes,
        excludes,
        include_patterns,
        exclude_patterns,
        aliases,
        included_matches,
        excluded_matches,
    })
}

fn below(root: &RelPath, path: &RelPath) -> Option<String> {
    path.strip_prefix(root)
        .filter(|rest| !rest.is_root())
        .map(|rest| rest.to_string())
}

fn entry_of(
    path: String,
    kind: Kind,
    size: u64,
    mtime_ns: i64,
    mode: u32,
    hash: Option<Hash>,
    link: Option<String>,
) -> ManifestEntry {
    ManifestEntry {
        p: path,
        t: kind.into(),
        s: if kind == Kind::File {
            i64::try_from(size).unwrap_or(i64::MAX)
        } else {
            0
        },
        m: mtime_ns,
        x: mode,
        h: if kind == Kind::File { hash } else { None },
        l: link,
    }
}

/// Lists what the selection names, as the tree is now. Hashes are the ones the cache knows for
/// the very version of each file.
pub fn collect(files: &Files, resolved: &Resolved) -> Result<Vec<ManifestEntry>> {
    crate::error::check_cancelled()?;
    let workspace = files.root();
    let cache = files.hashes();
    let mut found: BTreeMap<String, ManifestEntry> = BTreeMap::new();
    for include in &resolved.includes {
        if resolved.excluded(include) {
            continue;
        }
        let located = workspace.locate(include, Locate::ENTRY)?;
        let Some(meta) = located.meta else { continue };
        if let Some(rules) = files.ignore()
            && rules.is_ignored(workspace, include, meta.is_dir())
        {
            continue;
        }
        if !include.is_root()
            && resolved.included(include)
            && let Some(path) = below(&resolved.root, include)
        {
            let hash = meta
                .is_file()
                .then(|| cache.get(include, &meta.identity()))
                .flatten();
            let link = if meta.is_symlink() {
                located
                    .name
                    .as_deref()
                    .and_then(|name| located.parent.read_link(name).ok())
                    .map(|t| t.to_string_lossy().into_owned())
            } else {
                None
            };
            found.insert(
                path.clone(),
                entry_of(
                    path,
                    meta.kind,
                    meta.size,
                    meta.mtime_ns,
                    meta.mode,
                    hash,
                    link,
                ),
            );
        }
        if !meta.is_dir() {
            continue;
        }
        walk(workspace, files.ignore(), include, &mut |entry| {
            if resolved.excluded(&entry.path) {
                return Flow::SkipDir;
            }
            if !resolved.included(&entry.path) {
                return Flow::Continue;
            }
            if let Some(path) = below(&resolved.root, &entry.path) {
                let hash = entry
                    .meta
                    .is_file()
                    .then(|| cache.get(&entry.path, &entry.meta.identity()))
                    .flatten();
                found.insert(
                    path.clone(),
                    entry_of(
                        path,
                        entry.meta.kind,
                        entry.meta.size,
                        entry.meta.mtime_ns,
                        entry.meta.mode,
                        hash,
                        entry.link_target.clone(),
                    ),
                );
            }
            Flow::Continue
        })?;
    }
    // Keep only the ancestors needed to place matched entries in their directories. These
    // carry no file content, and unmatched siblings never become part of the snapshot.
    let selected: Vec<String> = found.keys().cloned().collect();
    for path in selected {
        let mut parent = RelPath::parse(&path, PathStyle::Posix)?.parent();
        while let Some(relative) = parent.filter(|parent| !parent.is_root()) {
            if !found.contains_key(relative.as_str()) {
                let full = if resolved.root.is_root() {
                    relative.clone()
                } else {
                    workspace.parse(&format!("{}/{relative}", resolved.root))?
                };
                let located = workspace.locate(&full, Locate::ENTRY)?;
                if let Some(meta) = located.meta.filter(|meta| meta.is_dir()) {
                    found.insert(
                        relative.to_string(),
                        entry_of(
                            relative.to_string(),
                            meta.kind,
                            meta.size,
                            meta.mtime_ns,
                            meta.mode,
                            None,
                            None,
                        ),
                    );
                }
            }
            parent = relative.parent();
        }
    }
    Ok(found.into_values().collect())
}

/// The entries of the index, with the hashes the cache knows. The index holds the whole tree.
#[must_use]
pub fn entries_of_index(
    index: &Index,
    cache: &HashCache,
    workspace: &Workspace,
) -> Vec<ManifestEntry> {
    index
        .entries()
        .into_iter()
        .map(|(path, entry): (RelPath, IndexEntry)| {
            let hash = if entry.kind == Kind::File {
                workspace
                    .locate(&path, Locate::ENTRY)
                    .ok()
                    .and_then(|located| located.meta)
                    .and_then(|meta| cache.get(&path, &meta.identity()))
            } else {
                None
            };
            entry_of(
                path.to_string(),
                entry.kind,
                entry.size,
                entry.mtime_ns,
                entry.mode,
                hash,
                entry.link_target,
            )
        })
        .collect()
}

/// The header of a manifest made now.
#[must_use]
pub fn header_for(
    workspace: &Workspace,
    root: &RelPath,
    generation: i64,
    now: DateTime<Utc>,
) -> ManifestHeader {
    ManifestHeader {
        v: MANIFEST_VERSION,
        root: root
            .under(workspace.root_path())
            .to_string_lossy()
            .into_owned(),
        generated_at: Time::new(now),
        generation,
        case_sensitive: workspace.case_sensitive(),
    }
}

/// Why a manifest is worth publishing now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// A turn completed and the tree changed.
    TurnCompleted,
    /// Changes have gone on for a whole interval since the last one.
    Interval,
}

/// Decides when the manifest of the whole tree is published.
#[derive(Debug)]
pub struct Publisher {
    clock: Arc<dyn Clock>,
    interval: Duration,
    state: Mutex<PublisherState>,
}

#[derive(Debug, Default)]
struct PublisherState {
    last: Option<DateTime<Utc>>,
    turn_completed: bool,
}

impl Publisher {
    /// A publisher that publishes at most once in `interval` while changes go on, and at once for
    /// a turn that completes.
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>, interval: Duration) -> Self {
        Self {
            clock,
            interval,
            state: Mutex::new(PublisherState::default()),
        }
    }

    /// Says that a turn completed.
    pub fn turn_completed(&self) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .turn_completed = true;
    }

    /// Whether a manifest is due now, given whether the tree changed since the last one.
    #[must_use]
    pub fn due(&self, tree_changed: bool) -> Option<Reason> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        // An empty complete index is still a tree. Without this first publication its
        // server-side names/search cache stays absent forever, however long the host is idle.
        let Some(last) = state.last else {
            return Some(Reason::Interval);
        };
        if !tree_changed {
            return None;
        }
        if state.turn_completed {
            return Some(Reason::TurnCompleted);
        }
        let waited = self.clock.now().signed_duration_since(last);
        (waited.to_std().is_ok_and(|waited| waited >= self.interval)).then_some(Reason::Interval)
    }

    /// Says that a manifest was published just now.
    pub fn published(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.last = Some(self.clock.now());
        state.turn_completed = false;
    }
}

/// The expanded lines of a manifest, for tests that compare with the text.
#[cfg(test)]
#[cfg(unix)]
fn expanded_lines(bytes: &[u8]) -> Vec<String> {
    let Ok(decoder) = zstd::stream::read::Decoder::new(bytes) else {
        return Vec::new();
    };
    BufReader::new(decoder)
        .lines()
        .map_while(std::result::Result::ok)
        .collect()
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crate::environment::{GateState, Gates, Limits};
    use crate::ignore::IgnoreRules;
    use orch_types::ManualClock;

    fn files_with(root: &std::path::Path, patterns: &[&str]) -> Files {
        let ws = Workspace::open(root, &[], PathStyle::Posix, true).unwrap();
        let rules = Arc::new(IgnoreRules::new(
            &patterns.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>(),
            true,
        ));
        Files::workspace(
            ws,
            rules,
            Arc::new(HashCache::new()),
            Limits::default(),
            Arc::new(GateState::new(Gates::OPEN)),
        )
    }

    fn selection(include: &[&str], exclude: &[&str]) -> Selection {
        Selection {
            include: include.iter().map(|s| (*s).to_owned()).collect(),
            exclude: exclude.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    fn tree() -> (tempfile::TempDir, Files) {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("web/src")).unwrap();
        std::fs::create_dir_all(root.path().join("a")).unwrap();
        std::fs::write(root.path().join("web/src/app.ts"), "app").unwrap();
        std::fs::write(root.path().join("web/readme.md"), "readme").unwrap();
        std::fs::write(root.path().join("web/skip.log"), "log").unwrap();
        std::fs::write(root.path().join("a/c"), "c").unwrap();
        std::fs::write(root.path().join("a-b"), "ab").unwrap();
        std::os::unix::fs::symlink("web/readme.md", root.path().join("link")).unwrap();
        let files = files_with(root.path(), &["*.log"]);
        (root, files)
    }

    #[test]
    fn an_explicit_directory_alias_selects_the_same_files_and_exclusions_as_its_target() {
        let (root, files) = tree();
        std::os::unix::fs::symlink("web", root.path().join("shortcut")).unwrap();
        let equivalent = |direct: Selection, alias: Selection| {
            let direct = resolve(files.root(), &direct).unwrap();
            let alias = resolve(files.root(), &alias).unwrap();
            let expected = collect(&files, &direct).unwrap();
            assert!(expected.iter().any(|entry| entry.t == EntryKind::File));
            assert_eq!(alias.root, direct.root);
            assert_eq!(collect(&files, &alias).unwrap(), expected);
        };
        equivalent(selection(&["web"], &[]), selection(&["shortcut"], &[]));
        equivalent(
            selection(&["web/**/*.ts"], &[]),
            selection(&["shortcut/**/*.ts"], &[]),
        );
        equivalent(
            selection(&["web"], &["web/src"]),
            selection(&["shortcut"], &["shortcut/src"]),
        );
        equivalent(
            selection(&["web"], &["web/**/app.ts"]),
            selection(&["shortcut"], &["shortcut/**/app.ts"]),
        );

        let whole = collect(
            &files,
            &resolve(files.root(), &selection(&[], &[])).unwrap(),
        )
        .unwrap();
        assert!(
            whole
                .iter()
                .any(|entry| entry.p == "shortcut" && entry.t == EntryKind::Link)
        );
        assert!(whole.iter().all(|entry| !entry.p.starts_with("shortcut/")));
        let without_link = collect(
            &files,
            &resolve(files.root(), &selection(&[], &["shortcut"])).unwrap(),
        )
        .unwrap();
        assert!(without_link.iter().all(|entry| entry.p != "shortcut"));
        assert!(
            without_link
                .iter()
                .any(|entry| entry.p == "web/src/app.ts" && entry.t == EntryKind::File)
        );
    }

    #[test]
    fn a_manifest_is_sorted_by_path_byte_by_byte_and_reads_back() {
        let (root, files) = tree();
        let resolved = resolve(files.root(), &selection(&[], &[])).unwrap();
        assert!(resolved.is_whole_tree());
        let entries = collect(&files, &resolved).unwrap();
        let header = header_for(
            files.root(),
            &resolved.root,
            3,
            chrono::DateTime::from_timestamp(1_791_484_919, 0).unwrap(),
        );
        let manifest = build(&header, entries).unwrap();
        let lines = expanded_lines(&manifest.bytes);
        assert!(lines[0].starts_with("{\"v\":1,\"root\":\""), "{}", lines[0]);
        assert!(
            lines[0].contains("\"generation\":3") && lines[0].contains("\"case_sensitive\":true")
        );
        let paths: Vec<String> = lines[1..]
            .iter()
            .map(|l| {
                serde_json::from_str::<serde_json::Value>(l).unwrap()["p"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(
            paths,
            [
                "a",
                "a-b",
                "a/c",
                "link",
                "web",
                "web/readme.md",
                "web/src",
                "web/src/app.ts"
            ],
            "'-' sorts before '/', and *.log is left out"
        );
        assert_eq!(
            (manifest.file_count, manifest.total_bytes),
            (4, 1 + 2 + 6 + 3)
        );
        assert_eq!(manifest.hash, Hash::of(&manifest.bytes));

        let parsed = parse(&manifest.bytes).unwrap();
        assert_eq!(parsed.header, header);
        let link = parsed.entries.iter().find(|e| e.p == "link").unwrap();
        assert_eq!(
            (link.t, link.l.as_deref()),
            (EntryKind::Link, Some("web/readme.md"))
        );
        let dir = parsed.entries.iter().find(|e| e.p == "web").unwrap();
        assert_eq!((dir.t, dir.s, dir.h), (EntryKind::Dir, 0, None));
        assert_eq!(
            parsed.header.root,
            root.path().canonicalize().unwrap().to_string_lossy()
        );
    }

    #[test]
    fn a_hash_is_in_the_manifest_only_where_it_is_known_for_that_very_version() {
        let (root, files) = tree();
        let located = files
            .root()
            .locate(
                &RelPath::parse("web/readme.md", PathStyle::Posix).unwrap(),
                Locate::THROUGH,
            )
            .unwrap();
        files.hashes().put(
            &located.real,
            located.meta.unwrap().identity(),
            Hash::of(b"readme"),
        );
        let resolved = resolve(files.root(), &selection(&["web"], &[])).unwrap();
        let entries = collect(&files, &resolved).unwrap();
        let by_path: BTreeMap<&str, &ManifestEntry> =
            entries.iter().map(|e| (e.p.as_str(), e)).collect();
        assert_eq!(by_path["readme.md"].h, Some(Hash::of(b"readme")));
        assert_eq!(by_path["src/app.ts"].h, None);
        // The file is written again; what was known of the old version is not believed of the new.
        // A different size establishes a new identity even on filesystems with coarse timestamps.
        std::fs::write(root.path().join("web/readme.md"), "new README").unwrap();
        let again = collect(&files, &resolved).unwrap();
        assert_eq!(again.iter().find(|e| e.p == "readme.md").unwrap().h, None);
    }

    #[test]
    fn a_selection_of_one_directory_is_written_relative_to_it() {
        let (root, files) = tree();
        let resolved = resolve(files.root(), &selection(&["web"], &["web/src"])).unwrap();
        assert_eq!(resolved.root.as_str(), "web");
        let entries = collect(&files, &resolved).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.p.as_str()).collect();
        assert_eq!(
            paths,
            ["readme.md"],
            "web/src is left out and the paths begin below web"
        );
        let header = header_for(
            files.root(),
            &resolved.root,
            1,
            chrono::DateTime::from_timestamp(0, 0).unwrap(),
        );
        assert_eq!(
            header.root,
            root.path()
                .canonicalize()
                .unwrap()
                .join("web")
                .to_string_lossy()
        );

        let several = resolve(files.root(), &selection(&["web/src", "a"], &[])).unwrap();
        assert_eq!(
            several.root,
            RelPath::root(),
            "several places are written from the workspace root"
        );
        let paths: Vec<String> = collect(&files, &several)
            .unwrap()
            .into_iter()
            .map(|e| e.p)
            .collect();
        assert_eq!(
            paths,
            ["a", "a/c", "web", "web/src", "web/src/app.ts"],
            "the selected nested directory keeps its parent metadata, without its unmatched siblings"
        );
        let missing = resolve(files.root(), &selection(&["nowhere"], &[])).unwrap();
        assert!(
            collect(&files, &missing).unwrap().is_empty(),
            "a place that is not there is not part of it"
        );
    }

    #[test]
    fn a_selection_cannot_reach_outside_the_workspace_or_into_what_is_left_out() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "s").unwrap();
        let (root, files) = tree();
        std::os::unix::fs::symlink(outside.path(), root.path().join("out")).unwrap();
        assert!(resolve(files.root(), &selection(&["../x"], &[])).is_err());
        let through = resolve(files.root(), &selection(&["out"], &[])).unwrap();
        assert!(
            through.includes.is_empty(),
            "a link that leaves the workspace names nothing"
        );
        let log = resolve(files.root(), &selection(&["web/skip.log"], &[])).unwrap();
        assert!(
            collect(&files, &log).unwrap().is_empty(),
            "a file that is left out cannot be named either"
        );
    }

    #[test]
    fn a_manifest_that_is_not_one_is_refused_and_so_is_a_path_it_may_not_hold() {
        assert!(parse(b"not zstd").is_err());
        let header = ManifestHeader {
            v: 1,
            root: "/w".into(),
            generated_at: Time::new(chrono::DateTime::from_timestamp(0, 0).unwrap()),
            generation: 1,
            case_sensitive: true,
        };
        let escaping = ManifestEntry {
            p: "../../etc/passwd".into(),
            t: EntryKind::File,
            s: 0,
            m: 0,
            x: 0o644,
            h: None,
            l: None,
        };
        let blob = build(&header, vec![escaping]).unwrap();
        assert!(
            parse(&blob.bytes).is_err(),
            "a manifest from another machine is not trusted with its paths"
        );
        let other_version = ManifestHeader { v: 2, ..header };
        assert!(parse(&build(&other_version, vec![]).unwrap().bytes).is_err());
    }

    #[test]
    fn a_manifest_is_due_when_a_turn_completed_or_changes_have_gone_on_for_the_interval() {
        let clock = ManualClock::new(chrono::DateTime::from_timestamp(1_000_000, 0).unwrap());
        let publisher = Publisher::new(Arc::new(clock.clone()), Duration::from_secs(300));
        assert_eq!(
            publisher.due(false),
            Some(Reason::Interval),
            "even an empty first tree must be published"
        );
        assert_eq!(
            publisher.due(true),
            Some(Reason::Interval),
            "the first one is not held back"
        );
        publisher.published();
        assert_eq!(
            publisher.due(false),
            None,
            "an unchanged published tree sends nothing"
        );
        assert_eq!(publisher.due(true), None);
        clock.advance(chrono::Duration::seconds(299));
        assert_eq!(publisher.due(true), None);
        clock.advance(chrono::Duration::seconds(1));
        assert_eq!(
            publisher.due(true),
            Some(Reason::Interval),
            "five minutes of changes"
        );
        publisher.published();
        publisher.turn_completed();
        assert_eq!(
            publisher.due(true),
            Some(Reason::TurnCompleted),
            "a turn that completes does not wait"
        );
        assert_eq!(publisher.due(false), None, "but only if the tree changed");
        publisher.published();
        assert_eq!(publisher.due(true), None);
    }

    #[test]
    fn a_manifest_of_two_hundred_thousand_entries_is_built_and_read_whole() {
        let header = ManifestHeader {
            v: 1,
            root: "/w".into(),
            generated_at: Time::new(chrono::DateTime::from_timestamp(0, 0).unwrap()),
            generation: 1,
            case_sensitive: true,
        };
        let entries: Vec<ManifestEntry> = (0..200_000)
            .map(|n| ManifestEntry {
                p: format!("dir{:03}/sub{:02}/file{n:06}.rs", n % 500, n % 37),
                t: EntryKind::File,
                s: 100,
                m: 1,
                x: 0o644,
                h: None,
                l: None,
            })
            .collect();
        let started = std::time::Instant::now();
        let blob = build(&header, entries).unwrap();
        let parsed = parse(&blob.bytes).unwrap();
        assert_eq!(parsed.entries.len(), 200_000);
        assert_eq!(blob.file_count, 200_000);
        eprintln!(
            "built and read 200000 complete entries in {:?}",
            started.elapsed()
        );
    }
}
