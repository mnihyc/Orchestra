//! Immediate children of a directory in an archive listing.

use super::{ArchiveEntry, ArchiveEntryKind};
use orch_types::Time;
use std::collections::BTreeMap;

/// An entry's display path without leading slashes, leading `./` or a trailing slash.
pub fn clean(path: &str) -> &str {
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

/// The immediate children gathered from entries in archive order.
pub struct Children {
    inside: String,
    limit: usize,
    found: BTreeMap<String, ArchiveEntry>,
}

impl Children {
    /// Starts a listing at the root or at the given directory.
    pub fn new(inside: Option<&str>, limit: usize) -> Self {
        Self {
            inside: inside.map(clean).unwrap_or("").to_owned(),
            limit,
            found: BTreeMap::new(),
        }
    }

    /// Notes an entry; says whether more than the limit are known now.
    pub fn add(
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

    /// Returns directories first, then files, and whether more children were seen.
    pub fn finish(self) -> (Vec<ArchiveEntry>, bool) {
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
