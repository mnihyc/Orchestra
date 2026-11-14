//! File version and on-demand hash cache; live-only mode for a tree too large to index.
//!
//! The index remembers, for every path of the workspace, what a listing needs: what it is, its
//! size, its modification time and its permission bits. It is built by one walk and kept current
//! from the paths the watcher reports, and from a full walk again when watching falls short. A
//! tree that holds more entries than the limit is not indexed at all: the index then says it is
//! [`IndexState::Live`], keeps nothing, and every listing is read from the disk when asked.
//!
//! Hashes are not in the index. They are computed when a file is fetched or snapshotted and
//! remembered by the [`HashCache`], which trusts a hash only while the file is the very version it
//! was computed for: same file, same size, same modification time and same change time, so a
//! program that writes new content and sets the modification time back is not believed.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use orch_link_contract::commands::IndexMode;
use orch_types::Hash;

use crate::error::Result;
use crate::ignore::IgnoreRules;
use crate::paths::{Locate, RelPath, Workspace};
use crate::platform::{Identity, Kind, Meta};
use crate::walk::{Flow, walk};

/// How many hashes the cache remembers. A hash is 32 bytes and a path about as many again; a
/// million entries is under a hundred megabytes, which is also far more files than a person's
/// session fetches. When the cache is full it is emptied: a hash is cheap to compute again next to
/// the cost of keeping a smarter structure.
pub const HASH_CACHE_MAX_ENTRIES: usize = 1_000_000;

/// Standalone readers use the same shipped scan slice; served environments supply resolved settings.
#[must_use]
pub fn default_scan_max() -> Duration {
    Duration::from_secs(orch_link_contract::commands::WorkspaceLimits::default().index_scan_secs)
}

/// Hashes of files, valid while the file is the version they were computed for.
#[derive(Debug, Default)]
pub struct HashCache {
    entries: Mutex<HashMap<RelPath, (Identity, Hash)>>,
}

impl HashCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<RelPath, (Identity, Hash)>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The hash of `path` if it was computed for exactly this version of the file.
    #[must_use]
    pub fn get(&self, path: &RelPath, version: &Identity) -> Option<Hash> {
        // Windows does not expose a reliable change time or file identity here. Size and
        // modification time alone can be restored after an edit, so reuse would return stale bytes.
        #[cfg(windows)]
        {
            let _ = (path, version);
            None
        }
        #[cfg(not(windows))]
        {
            self.entries()
                .get(path)
                .filter(|(known, _)| known == version)
                .map(|(_, hash)| *hash)
        }
    }

    /// Remembers the hash of `path` for this version of the file.
    pub fn put(&self, path: &RelPath, version: Identity, hash: Hash) {
        #[cfg(windows)]
        {
            let _ = (path, version, hash);
        }
        #[cfg(not(windows))]
        {
            let mut entries = self.entries();
            if entries.len() >= HASH_CACHE_MAX_ENTRIES && !entries.contains_key(path) {
                entries.clear();
            }
            entries.insert(path.clone(), (version, hash));
        }
    }

    /// Forgets what is known of `path`.
    pub fn forget(&self, path: &RelPath) {
        self.entries().remove(path);
    }

    /// How many hashes are remembered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries().len()
    }

    /// Whether nothing is remembered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries().is_empty()
    }
}

/// What the index knows of one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    /// What it is.
    pub kind: Kind,
    /// Its size in bytes; zero for anything but a file.
    pub size: u64,
    /// When its content changed, in nanoseconds since 1970.
    pub mtime_ns: i64,
    /// Its permission bits.
    pub mode: u32,
    /// For a link, where it points as written.
    pub link_target: Option<String>,
}

impl IndexEntry {
    fn of(meta: &Meta, link_target: Option<String>) -> Self {
        Self {
            kind: meta.kind,
            size: if meta.is_file() { meta.size } else { 0 },
            mtime_ns: meta.mtime_ns,
            mode: meta.mode,
            link_target,
        }
    }
}

/// Why a tree is not indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveReason {
    /// The setting `workspace.index` says so.
    Requested,
    /// The tree holds more entries than the limit.
    TooManyEntries {
        /// The limit.
        limit: u64,
    },
    /// The platform could not watch the whole tree.
    Watches,
    /// A single scan exceeded the configured time budget.
    ScanTime,
    /// Some workspace metadata or exclusion rules could not be read.
    Unreadable,
}

/// Whether the index is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexState {
    /// Every path is in the index.
    Full,
    /// A bounded working set is kept without whole-tree coverage, for the reason given.
    Live(LiveReason),
}

impl IndexState {
    /// The word the environment reports its index as.
    #[must_use]
    pub fn mode(self) -> IndexMode {
        match self {
            Self::Full => IndexMode::Full,
            Self::Live(_) => IndexMode::Live,
        }
    }

    /// Whether the index is kept.
    #[must_use]
    pub fn is_full(self) -> bool {
        self == Self::Full
    }

    /// The actual entry bound which made this index Live, when it crossed one.
    #[must_use]
    pub fn entry_limit(self) -> Option<u64> {
        match self {
            Self::Live(LiveReason::TooManyEntries { limit }) => Some(limit),
            _ => None,
        }
    }

    /// The stable reason a live tree reports to the server.
    #[must_use]
    pub fn reason(self) -> Option<orch_link_contract::events::IndexReason> {
        use orch_link_contract::events::IndexReason;
        match self {
            Self::Full => None,
            Self::Live(LiveReason::Requested) => Some(IndexReason::Chosen),
            Self::Live(LiveReason::TooManyEntries { .. }) => Some(IndexReason::Entries),
            Self::Live(LiveReason::Watches) => Some(IndexReason::Watches),
            Self::Live(LiveReason::ScanTime) => Some(IndexReason::ScanTime),
            Self::Live(LiveReason::Unreadable) => Some(IndexReason::Unreadable),
        }
    }
}

#[derive(Debug)]
struct Inner {
    state: IndexState,
    entries: BTreeMap<String, IndexEntry>,
    ages: BTreeMap<String, u64>,
    oldest: BTreeMap<u64, String>,
    age: u64,
    dirty: bool,
    ready: bool,
    revision: u64,
    known_since: u64,
    changes: BTreeMap<RelPath, u64>,
    change_order: BTreeMap<u64, RelPath>,
}

impl Inner {
    fn remember(&mut self, path: String, entry: IndexEntry, maximum: u64) -> bool {
        let changed = self.entries.insert(path.clone(), entry.clone()).as_ref() != Some(&entry);
        if let Some(age) = self.ages.remove(&path) {
            self.oldest.remove(&age);
        }
        // Exhausting the ordinal is a representation limit, not ordinary cache pressure.
        if self.age == u64::MAX {
            self.ages.clear();
            self.oldest.clear();
            self.age = 0;
            for path in self.entries.keys() {
                self.age += 1;
                self.ages.insert(path.clone(), self.age);
                self.oldest.insert(self.age, path.clone());
            }
        }
        self.age += 1;
        self.ages.insert(path.clone(), self.age);
        self.oldest.insert(self.age, path);
        while self.entries.len() as u64 > maximum {
            if self.state.is_full() {
                self.state = IndexState::Live(LiveReason::TooManyEntries { limit: maximum });
                self.ready = false;
            }
            if let Some((_, path)) = self.oldest.pop_first() {
                self.ages.remove(&path);
                self.entries.remove(&path);
            }
        }
        changed
    }

    fn remove(&mut self, path: &str) -> bool {
        if let Some(age) = self.ages.remove(path) {
            self.oldest.remove(&age);
        }
        self.entries.remove(path).is_some()
    }

    fn advance(&mut self) -> Option<u64> {
        let Some(revision) = self.revision.checked_add(1) else {
            self.state = IndexState::Live(LiveReason::Watches);
            self.changes.clear();
            self.change_order.clear();
            self.ready = false;
            self.dirty = false;
            return None;
        };
        self.revision = revision;
        Some(revision)
    }
}

/// The index of one workspace.
#[derive(Debug)]
pub struct Index {
    max_entries: u64,
    scan_max: Duration,
    scan: Mutex<()>,
    inner: Mutex<Inner>,
}

/// What a scan found out about the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanOutcome {
    /// Whether the tree is different from what the index held before.
    pub changed: bool,
    /// Whether the scan turned the index live because the tree is too large.
    pub went_live: bool,
}

impl Index {
    /// An index that is kept when `requested` says so, until the tree holds more than
    /// `max_entries` entries.
    #[must_use]
    pub fn new(requested: IndexMode, max_entries: u64) -> Self {
        Self::with_scan_limit(requested, max_entries, default_scan_max())
    }

    /// An index bounded by both entry count and the duration of one full scan.
    #[must_use]
    pub fn with_scan_limit(requested: IndexMode, max_entries: u64, scan_max: Duration) -> Self {
        let state = match requested {
            IndexMode::Full => IndexState::Full,
            IndexMode::Live => IndexState::Live(LiveReason::Requested),
        };
        Self {
            max_entries,
            scan_max,
            scan: Mutex::new(()),
            inner: Mutex::new(Inner {
                state,
                entries: BTreeMap::new(),
                ages: BTreeMap::new(),
                oldest: BTreeMap::new(),
                age: 0,
                dirty: false,
                ready: false,
                revision: 0,
                known_since: 0,
                changes: BTreeMap::new(),
                change_order: BTreeMap::new(),
            }),
        }
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the index is kept, and if not why.
    #[must_use]
    pub fn state(&self) -> IndexState {
        self.inner().state
    }

    /// Stops whole-tree work while retaining bounded metadata and the first coverage reason.
    pub fn give_up(&self, reason: LiveReason) {
        let mut inner = self.inner();
        if inner.state.is_full() {
            inner.state = IndexState::Live(reason);
            inner.changes.clear();
            inner.change_order.clear();
            inner.dirty = false;
            inner.ready = false;
        }
    }

    /// The watcher cursor a turn remembers before it starts.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.inner().revision
    }

    /// Paths reported by the watcher since a cursor; none when events were lost or the tree is live.
    /// This reads memory only, and never lists a directory or invokes git.
    #[must_use]
    pub fn changes_since(&self, cursor: u64) -> Option<Vec<RelPath>> {
        let inner = self.inner();
        if !inner.state.is_full()
            || !inner.ready
            || cursor < inner.known_since
            || cursor > inner.revision
        {
            return None;
        }
        if cursor == inner.revision {
            return Some(Vec::new());
        }
        let mut paths: Vec<_> = inner
            .change_order
            .range((
                std::ops::Bound::Excluded(cursor),
                std::ops::Bound::Unbounded,
            ))
            .map(|(_, path)| path.clone())
            .collect();
        paths.sort();
        Some(paths)
    }

    /// Records the last revision of each watcher path, bounded by distinct paths, not write rate.
    /// The ordered revision index lets eviction cost logarithmic work too.
    pub fn observe(&self, paths: &[RelPath]) {
        let mut inner = self.inner();
        if !inner.state.is_full() {
            return;
        }
        for path in paths {
            let Some(revision) = inner.advance() else {
                return;
            };
            if let Some(previous) = inner.changes.insert(path.clone(), revision) {
                inner.change_order.remove(&previous);
            }
            inner.change_order.insert(revision, path.clone());
            while inner.changes.len() as u64 > self.max_entries {
                if let Some((revision, path)) = inner.change_order.pop_first() {
                    inner.changes.remove(&path);
                    inner.known_since = revision;
                }
            }
        }
    }

    /// Marks a gap in the event stream, so no turn is promised a complete list across it.
    pub fn lost_events(&self) {
        let mut inner = self.inner();
        if inner.advance().is_none() {
            return;
        }
        inner.known_since = inner.revision;
        inner.changes.clear();
        inner.change_order.clear();
        inner.ready = false;
    }

    /// How many paths are in the index.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner().entries.len()
    }

    /// Whether the index holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// What the index knows of `path`.
    #[must_use]
    pub fn get(&self, path: &RelPath) -> Option<IndexEntry> {
        self.inner().entries.get(path.as_str()).cloned()
    }

    /// What the index knows of everything strictly below `path`, in the order of the paths: the
    /// cost is what lies there, not the size of the tree.
    #[must_use]
    pub fn below(&self, path: &RelPath) -> Vec<(RelPath, IndexEntry)> {
        let inner = self.inner();
        if path.is_root() {
            return inner
                .entries
                .iter()
                .map(|(path, entry)| (RelPath::from_walked(path.clone()), entry.clone()))
                .collect();
        }
        // Everything below "a/b" sorts between "a/b/" and "a/b0" ('0' follows '/').
        let from = format!("{}/", path.as_str());
        let to = format!("{}0", path.as_str());
        inner
            .entries
            .range(from..to)
            .map(|(path, entry)| (RelPath::from_walked(path.clone()), entry.clone()))
            .collect()
    }

    /// Every path of the index with what is known of it, in the order of the paths.
    #[must_use]
    pub fn entries(&self) -> Vec<(RelPath, IndexEntry)> {
        self.inner()
            .entries
            .iter()
            .map(|(path, entry)| (RelPath::from_walked(path.clone()), entry.clone()))
            .collect()
    }

    /// Says the tree changed, so that the next manifest is worth publishing.
    pub fn mark_dirty(&self) {
        self.inner().dirty = true;
    }

    /// Whether the tree changed since the last time this was taken; clears the flag.
    pub fn take_dirty(&self) -> bool {
        std::mem::take(&mut self.inner().dirty)
    }

    /// Whether the tree changed since the last time it was taken.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.inner().dirty
    }

    /// Walks the whole tree and makes the index what the walk found. A tree with more entries than
    /// the limit makes the index live: the walk stops as soon as it has counted past the limit.
    pub fn scan(&self, workspace: &Workspace, rules: &Arc<IgnoreRules>) -> Result<ScanOutcome> {
        let Ok(_scan) = self.scan.try_lock() else {
            return Ok(ScanOutcome {
                changed: false,
                went_live: false,
            });
        };
        if matches!(self.state(), IndexState::Live(_)) {
            return Ok(ScanOutcome {
                changed: false,
                went_live: false,
            });
        }
        let mut found: BTreeMap<String, IndexEntry> = BTreeMap::new();
        let limit = self.max_entries;
        let began = Instant::now();
        let mut reason = None;
        let outcome = walk(workspace, Some(rules), &RelPath::root(), &mut |entry| {
            if began.elapsed() >= self.scan_max || !self.state().is_full() {
                reason = Some(LiveReason::ScanTime);
                return Flow::Stop;
            }
            found.insert(
                entry.path.to_string(),
                IndexEntry::of(&entry.meta, entry.link_target.clone()),
            );
            if found.len() as u64 > limit {
                reason = Some(LiveReason::TooManyEntries { limit });
                Flow::Stop
            } else {
                Flow::Continue
            }
        })?;
        if began.elapsed() >= self.scan_max && reason.is_none() {
            reason = Some(LiveReason::ScanTime);
        }
        if outcome.stopped || reason.is_some() {
            self.give_up(reason.unwrap_or(LiveReason::ScanTime));
            let mut inner = self.inner();
            for (path, entry) in found {
                inner.remember(path, entry, self.max_entries);
            }
            return Ok(ScanOutcome {
                changed: true,
                went_live: true,
            });
        }
        let mut inner = self.inner();
        // A watch failure can give the tree up while the scan's directory read is finishing.
        if !inner.state.is_full() {
            return Ok(ScanOutcome {
                changed: true,
                went_live: true,
            });
        }
        let changed = inner.entries != found;
        inner.ready = outcome.unreadable.is_empty();
        if changed {
            inner.entries.clear();
            inner.ages.clear();
            inner.oldest.clear();
            for (path, entry) in found {
                inner.remember(path, entry, self.max_entries);
            }
            inner.dirty = true;
        }
        Ok(ScanOutcome {
            changed,
            went_live: false,
        })
    }

    /// Looks at each of `paths` again and makes the index say what is there now: a path that is
    /// gone goes with everything under it, a directory that is new is walked, and anything the
    /// ignore rules leave out is not kept.
    pub fn refresh(
        &self,
        workspace: &Workspace,
        rules: &Arc<IgnoreRules>,
        paths: &[RelPath],
    ) -> Result<ScanOutcome> {
        let was_full = self.state().is_full();
        let mut changed = false;
        for path in paths {
            if path.is_root() {
                continue;
            }
            changed |= self.refresh_one(workspace, rules, path)?;
        }
        let mut inner = self.inner();
        if changed {
            inner.dirty = true;
        }
        Ok(ScanOutcome {
            changed,
            went_live: was_full && !inner.state.is_full(),
        })
    }

    fn refresh_one(
        &self,
        workspace: &Workspace,
        rules: &Arc<IgnoreRules>,
        path: &RelPath,
    ) -> Result<bool> {
        let found = match workspace.locate(path, Locate::ENTRY) {
            Ok(located) if located.meta.is_some() && located.real == *path => located,
            // Gone, or a path that goes through a link: the index holds real paths only.
            _ => return Ok(self.remove_tree(path)),
        };
        let Some(meta) = found.meta else {
            return Ok(false);
        };
        if rules.is_ignored(workspace, path, meta.is_dir()) {
            return Ok(self.remove_tree(path));
        }
        let link_target = match (meta.is_symlink(), found.name.as_deref()) {
            (true, Some(name)) => found
                .parent
                .read_link(name)
                .ok()
                .map(|t| t.to_string_lossy().into_owned()),
            _ => None,
        };
        let entry = IndexEntry::of(&meta, link_target);
        let (new, mut changed) = {
            let mut inner = self.inner();
            let new = !inner.entries.contains_key(path.as_str());
            (
                new,
                inner.remember(path.to_string(), entry, self.max_entries),
            )
        };
        if meta.is_dir() && new && self.state().is_full() {
            let mut added: Vec<(String, IndexEntry)> = Vec::new();
            let began = Instant::now();
            let held = self.len() as u64;
            let mut reason = None;
            walk(workspace, Some(rules), path, &mut |entry| {
                if began.elapsed() >= self.scan_max {
                    reason = Some(LiveReason::ScanTime);
                    return Flow::Stop;
                }
                if held.saturating_add(added.len() as u64) >= self.max_entries {
                    reason = Some(LiveReason::TooManyEntries {
                        limit: self.max_entries,
                    });
                    return Flow::Stop;
                }
                added.push((
                    entry.path.to_string(),
                    IndexEntry::of(&entry.meta, entry.link_target.clone()),
                ));
                Flow::Continue
            })?;
            if let Some(reason) = reason {
                self.give_up(reason);
            }
            let mut inner = self.inner();
            for (path, entry) in added {
                inner.remember(path, entry, self.max_entries);
            }
            changed = true;
        }
        Ok(changed)
    }

    /// Adds a visible directory page to the bounded live metadata window without a tree walk.
    pub fn remember_page(&self, entries: &[orch_api_contract::workspace::DirEntry]) {
        use orch_api_contract::workspace::DirEntryKind;
        let mut inner = self.inner();
        if inner.state.is_full() {
            return;
        }
        for entry in entries {
            let kind = match entry.kind {
                DirEntryKind::File => Kind::File,
                DirEntryKind::Dir => Kind::Dir,
                DirEntryKind::Symlink => Kind::Symlink,
                DirEntryKind::Other => Kind::Other,
            };
            inner.remember(
                entry.path.clone(),
                IndexEntry {
                    kind,
                    size: u64::try_from(entry.size).unwrap_or(0),
                    mtime_ns: entry.mtime.get().timestamp_nanos_opt().unwrap_or(0),
                    mode: entry.mode,
                    link_target: entry.link_target.clone(),
                },
                self.max_entries,
            );
        }
    }

    /// Removes `path` and everything under it; says whether anything was removed.
    fn remove_tree(&self, path: &RelPath) -> bool {
        let mut inner = self.inner();
        let mut removed = inner.remove(path.as_str());
        // Everything below "a/b" sorts between "a/b/" and "a/b0" ('0' follows '/').
        let from = format!("{}/", path.as_str());
        let to = format!("{}0", path.as_str());
        let below: Vec<String> = inner
            .entries
            .range(from..to)
            .map(|(key, _)| key.clone())
            .collect();
        removed |= !below.is_empty();
        for key in below {
            inner.remove(&key);
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::PathStyle;

    fn native_style() -> PathStyle {
        if cfg!(windows) {
            PathStyle::Windows
        } else {
            PathStyle::Posix
        }
    }
    use std::path::Path;

    fn workspace(root: &Path) -> Workspace {
        Workspace::open(root, &[], native_style(), true).unwrap()
    }

    fn path(text: &str) -> RelPath {
        RelPath::parse(text, native_style()).unwrap()
    }

    fn rules() -> Arc<IgnoreRules> {
        Arc::new(IgnoreRules::new(&["*.log".to_owned()], true))
    }

    #[test]
    fn a_scan_indexes_the_tree_without_what_the_rules_leave_out() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(root.path().join("debug.log"), "noise").unwrap();
        let ws = workspace(root.path());
        let index = Index::new(IndexMode::Full, 1000);
        let outcome = index.scan(&ws, &rules()).unwrap();
        assert!(outcome.changed && !outcome.went_live);
        let paths: Vec<String> = index
            .entries()
            .into_iter()
            .map(|(p, _)| p.to_string())
            .collect();
        assert_eq!(paths, ["src", "src/main.rs"]);
        assert_eq!(index.get(&path("src/main.rs")).unwrap().size, 12);
        assert!(index.take_dirty());
        assert!(!index.take_dirty(), "the flag is taken once");
        assert!(
            !index.scan(&ws, &rules()).unwrap().changed,
            "a second scan of the same tree finds nothing new"
        );
        assert!(!index.is_dirty());
    }

    #[test]
    fn a_scan_that_exceeds_its_budget_gives_up_and_a_live_tree_never_walks_again() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("one"), "x").unwrap();
        let index = Index::with_scan_limit(IndexMode::Full, 1000, Duration::ZERO);
        let outcome = index.scan(&workspace(root.path()), &rules()).unwrap();
        assert!(outcome.went_live);
        assert_eq!(index.state(), IndexState::Live(LiveReason::ScanTime));
        assert_eq!(
            index.state().reason(),
            Some(orch_link_contract::events::IndexReason::ScanTime)
        );
        assert!(index.is_empty());
        assert!(
            !index
                .scan(&workspace(root.path()), &rules())
                .unwrap()
                .changed
        );
    }

    #[test]
    fn a_turn_observes_only_watcher_paths_and_lost_or_live_events_are_unavailable() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("one"), "x").unwrap();
        let index = Index::new(IndexMode::Full, 1000);
        assert!(
            index.changes_since(0).is_none(),
            "the first scan is unfinished"
        );
        index.scan(&workspace(root.path()), &rules()).unwrap();
        let before = index.revision();
        let paths = vec![path("one"), path("new"), path("one")];
        index.observe(&paths);
        assert_eq!(
            index.changes_since(before).unwrap(),
            vec![path("new"), path("one")]
        );
        assert!(index.changes_since(index.revision()).unwrap().is_empty());
        index.lost_events();
        assert!(index.changes_since(before).is_none());
        index.scan(&workspace(root.path()), &rules()).unwrap();
        assert!(index.changes_since(index.revision()).is_some());
        index.give_up(LiveReason::Watches);
        assert!(index.changes_since(index.revision()).is_none());
        assert_eq!(
            index.state().reason(),
            Some(orch_link_contract::events::IndexReason::Watches)
        );
    }

    #[test]
    fn repeated_writes_keep_a_turns_cursor_until_distinct_changed_paths_are_lost() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("code"), "before").unwrap();
        let index = Index::new(IndexMode::Full, 2);
        index.scan(&workspace(root.path()), &rules()).unwrap();
        let before = index.revision();
        for _ in 0..20 {
            index.observe(&[path("code")]);
        }
        assert_eq!(
            index.changes_since(before),
            Some(vec![path("code")]),
            "one busy file does not evict the only path a turn needs"
        );
        index.observe(&[path("second")]);
        assert_eq!(
            index.changes_since(before),
            Some(vec![path("code"), path("second")])
        );
        index.observe(&[path("third")]);
        assert!(
            index.changes_since(before).is_none(),
            "losing a distinct path leaves unknown coverage"
        );
    }

    #[test]
    fn exhausted_watcher_revisions_never_promise_an_empty_change_list() {
        for lost in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let index = Index::new(IndexMode::Full, 2);
            index.scan(&workspace(root.path()), &rules()).unwrap();
            index.inner().revision = u64::MAX;
            if lost {
                index.lost_events();
            } else {
                index.observe(&[path("code")]);
            }
            assert!(index.changes_since(u64::MAX).is_none());
            assert!(
                !index.state().is_full(),
                "another attachment must supply a new journal"
            );
        }
    }

    #[test]
    fn a_large_tree_keeps_a_bounded_window_and_native_changes_refresh_it() {
        let root = tempfile::tempdir().unwrap();
        for n in 0..7 {
            std::fs::write(root.path().join(format!("f{n}")), "x").unwrap();
        }
        let ws = workspace(root.path());
        let index = Index::new(IndexMode::Full, 5);
        let outcome = index.scan(&ws, &rules()).unwrap();
        assert!(outcome.went_live);
        assert_eq!(
            index.state(),
            IndexState::Live(LiveReason::TooManyEntries { limit: 5 })
        );
        assert_eq!(index.state().mode(), IndexMode::Live);
        assert_eq!(index.len(), 5, "useful metadata survives partial coverage");
        std::fs::write(root.path().join("recent"), "new contents").unwrap();
        assert!(
            index
                .refresh(&ws, &rules(), &[path("recent")])
                .unwrap()
                .changed
        );
        assert_eq!(index.get(&path("recent")).unwrap().size, 12);
        assert_eq!(index.len(), 5, "new paths evict old metadata incrementally");
        std::fs::remove_file(root.path().join("recent")).unwrap();
        index.refresh(&ws, &rules(), &[path("recent")]).unwrap();
        assert!(index.get(&path("recent")).is_none());
        assert!(
            !index.state().is_full(),
            "a window never claims whole coverage"
        );
    }

    #[test]
    fn losing_recursive_watches_keeps_metadata_but_never_promises_complete_coverage() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("code"), "kept").unwrap();
        let ws = workspace(root.path());
        let index = Index::new(IndexMode::Full, 5);
        index.scan(&ws, &rules()).unwrap();
        index.give_up(LiveReason::Watches);
        assert_eq!(index.get(&path("code")).unwrap().size, 4);
        assert!(!index.state().is_full());
        assert!(index.changes_since(index.revision()).is_none());
    }

    #[test]
    fn an_index_asked_to_be_live_never_scans() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a"), "x").unwrap();
        let index = Index::new(IndexMode::Live, 1000);
        assert_eq!(index.state(), IndexState::Live(LiveReason::Requested));
        assert!(
            !index
                .scan(&workspace(root.path()), &rules())
                .unwrap()
                .changed
        );
        assert!(index.is_empty());
    }

    #[test]
    fn changes_the_watcher_reports_make_the_index_say_what_is_there_now() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("a/b")).unwrap();
        std::fs::write(root.path().join("a/b/f.txt"), "1").unwrap();
        let ws = workspace(root.path());
        let index = Index::new(IndexMode::Full, 1000);
        index.scan(&ws, &rules()).unwrap();
        index.take_dirty();

        // A file is written, a directory with files in it appears, and a directory goes.
        std::fs::write(root.path().join("a/new.txt"), "22").unwrap();
        std::fs::create_dir_all(root.path().join("c/d")).unwrap();
        std::fs::write(root.path().join("c/d/g.txt"), "333").unwrap();
        std::fs::write(root.path().join("c/skipped.log"), "x").unwrap();
        std::fs::remove_dir_all(root.path().join("a/b")).unwrap();
        let changes = [path("a/new.txt"), path("c"), path("a/b"), path("a/b/f.txt")];
        let outcome = index.refresh(&ws, &rules(), &changes).unwrap();
        assert!(outcome.changed);
        assert!(index.is_dirty());
        let paths: Vec<String> = index
            .entries()
            .into_iter()
            .map(|(p, _)| p.to_string())
            .collect();
        assert_eq!(paths, ["a", "a/new.txt", "c", "c/d", "c/d/g.txt"]);
        assert_eq!(index.get(&path("a/new.txt")).unwrap().size, 2);

        // Asking again about what has not changed changes nothing.
        index.take_dirty();
        assert!(
            !index
                .refresh(&ws, &rules(), &[path("a/new.txt")])
                .unwrap()
                .changed
        );
        assert!(!index.is_dirty());
    }

    #[test]
    fn a_hash_is_believed_only_for_the_very_version_it_was_computed_for() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("f.txt");
        std::fs::write(&file, "aaaa").unwrap();
        let dir = crate::platform::Dir::open_path(root.path()).unwrap();
        let name = std::ffi::OsStr::new("f.txt");
        let before = dir.lstat(name).unwrap();
        let cache = HashCache::new();
        let key = path("f.txt");
        cache.put(&key, before.identity(), Hash::of(b"aaaa"));
        #[cfg(not(windows))]
        assert_eq!(cache.get(&key, &before.identity()), Some(Hash::of(b"aaaa")));
        #[cfg(windows)]
        assert_eq!(cache.get(&key, &before.identity()), None);

        // New content of the same size, and the modification time put back: not the same version.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&file, "bbbb").unwrap();
        let original = std::time::UNIX_EPOCH
            + std::time::Duration::from_nanos(u64::try_from(before.mtime_ns).unwrap());
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(original)
            .unwrap();
        let after = dir.lstat(name).unwrap();
        assert_eq!(
            (after.size, after.mtime_ns),
            (before.size, before.mtime_ns),
            "the trick worked on size and time"
        );
        assert_eq!(
            cache.get(&key, &after.identity()),
            None,
            "an unverifiable version never returns the earlier hash"
        );
    }

    #[test]
    fn what_lies_below_a_path_is_read_without_the_rest_of_the_tree() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("a/b")).unwrap();
        std::fs::create_dir_all(root.path().join("a0")).unwrap();
        std::fs::write(root.path().join("a/one.txt"), "1").unwrap();
        std::fs::write(root.path().join("a/b/two.txt"), "2").unwrap();
        std::fs::write(root.path().join("a0/three.txt"), "3").unwrap();
        std::fs::write(root.path().join("top.txt"), "t").unwrap();
        let ws = workspace(root.path());
        let index = Index::new(IndexMode::Full, 1000);
        index.scan(&ws, &rules()).unwrap();
        let below: Vec<String> = index
            .below(&path("a"))
            .into_iter()
            .map(|(path, _)| path.to_string())
            .collect();
        assert_eq!(
            below,
            ["a/b", "a/b/two.txt", "a/one.txt"],
            "a sibling that sorts right after the directory is not below it"
        );
        assert!(index.below(&path("a/one.txt")).is_empty());
        assert_eq!(index.below(&RelPath::root()).len(), index.len());
    }
}
