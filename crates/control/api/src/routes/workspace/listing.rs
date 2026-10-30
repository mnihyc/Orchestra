//! Entries and file descriptions made from what the API kept: the list of a tree a machine uploaded, or the versions of files whose content it holds.
//!
//! A machine that is away is answered from these. A directory is read from the list by its path;
//! where the tree is too large to index there is no list, and what is shown of an environment that
//! is away is the files that were opened before. Each entry says whether its content is in the
//! store for exactly the version it describes, which is what the cache of file content knows.

use super::paths;
use crate::cache::{Entry, EntryKind, Manifest};
use crate::middleware::requests::ApiResult;
use orch_api_contract::workspace::{DirEntry, DirEntryKind, FileStat};
use orch_db::Conn;
use orch_db::queries::environments::workspace::EnvironmentRow;
use orch_db::queries::storage::{self, CachedFile, NANOS_PER_MILLI};
use orch_link_contract::commands::FS_LIST_MAX_ENTRIES;
use orch_types::{Hash, Id, Json, Time};
use std::collections::HashSet;

/// How many entries a listing of one directory holds: as many as a machine's own listing does, so
/// that a directory is cut at the same place whichever way it is read.
pub const LISTING_MAX_ENTRIES: usize = FS_LIST_MAX_ENTRIES as usize;

/// Whether a list of the environment's whole tree is kept: it is, unless the machine said that the
/// tree is too large for it.
#[must_use]
pub fn indexed(env: &EnvironmentRow) -> bool {
    env.facts.get("index").and_then(Json::as_str) != Some("live")
}

fn kind_of(kind: EntryKind) -> DirEntryKind {
    match kind {
        EntryKind::File => DirEntryKind::File,
        EntryKind::Dir => DirEntryKind::Dir,
        EntryKind::Link => DirEntryKind::Symlink,
        EntryKind::Other => DirEntryKind::Other,
    }
}

/// An entry of a list as the API shows an entry of a directory. Whether its content is cached is
/// not known from the list.
#[must_use]
pub fn dir_entry(entry: &Entry) -> DirEntry {
    DirEntry {
        name: entry.name().to_owned(),
        path: entry.path.clone(),
        kind: kind_of(entry.kind),
        size: entry.size,
        mtime: Time::new(entry.mtime()),
        mode: entry.mode,
        link_target: entry.link.clone(),
        cached: false,
    }
}

/// Puts directories first, and then orders by name without regard to case.
pub fn sort(entries: &mut [DirEntry]) {
    entries.sort_by(|a, b| {
        let a_dir = a.kind == DirEntryKind::Dir;
        let b_dir = b.kind == DirEntryKind::Dir;
        b_dir
            .cmp(&a_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// What lies directly in `directory` of a list; none if nothing is there, neither the directory nor
/// anything below it.
#[must_use]
pub fn from_manifest(tree: &Manifest, directory: &str) -> Option<Vec<DirEntry>> {
    if !directory.is_empty() {
        match tree.get(directory) {
            Some(entry) if entry.kind != EntryKind::Dir => return None,
            Some(_) => {}
            None if tree.has_below(directory) => {}
            None => return None,
        }
    }
    Some(tree.children(directory).iter().map(dir_entry).collect())
}

/// What lies directly in `directory`, made from the versions of files whose content is cached: a
/// file of the directory with its newest version, and a directory for each that something cached lies
/// below.
#[must_use]
pub fn from_cache(rows: &[CachedFile], directory: &str) -> Vec<DirEntry> {
    let prefix = if directory.is_empty() {
        String::new()
    } else {
        format!("{directory}/")
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut entries = Vec::new();
    for row in rows {
        let Some(rest) = row.path.strip_prefix(&prefix) else {
            continue;
        };
        let (name, is_dir) = match rest.split_once('/') {
            Some((name, _)) => (name, true),
            None => (rest, false),
        };
        // Rows come newest version first for each path: the first one is the one shown.
        if name.is_empty() || !seen.insert(format!("{name}/{is_dir}")) {
            continue;
        }
        entries.push(DirEntry {
            name: name.to_owned(),
            path: format!("{prefix}{name}"),
            kind: if is_dir {
                DirEntryKind::Dir
            } else {
                DirEntryKind::File
            },
            size: if is_dir { 0 } else { row.size },
            mtime: Time::new(chrono::DateTime::from_timestamp_nanos(row.mtime_ns)),
            mode: if is_dir { 0o755 } else { 0o644 },
            link_target: None,
            cached: !is_dir,
        });
    }
    entries
}

/// Sets `cached` on the files among `entries` whose version has its content in the store.
pub async fn mark_cached(
    conn: &mut Conn,
    environment: Id,
    mount: Option<&str>,
    entries: &mut [DirEntry],
) -> ApiResult<()> {
    let keys: Vec<String> = entries
        .iter()
        .filter(|entry| entry.kind == DirEntryKind::File)
        .map(|entry| paths::cache_key(mount, &entry.path))
        .collect();
    if keys.is_empty() {
        return Ok(());
    }
    let found = storage::cached_among(conn, environment, &keys).await?;
    for entry in entries
        .iter_mut()
        .filter(|entry| entry.kind == DirEntryKind::File)
    {
        let key = paths::cache_key(mount, &entry.path);
        let millis = entry.mtime.get().timestamp_millis();
        entry.cached = found
            .iter()
            .any(|(path, size, ms)| *path == key && *size == entry.size && *ms == millis);
    }
    Ok(())
}

/// What is known of a file or a directory without asking its machine: its entry, and the hash of
/// its content where the list had it.
#[must_use]
pub fn file_stat(entry: DirEntry, hash: Option<Hash>) -> FileStat {
    let (media_type, is_text) = if entry.kind == DirEntryKind::Dir {
        (crate::media::DIRECTORY.to_owned(), false)
    } else {
        let media = crate::media::media_type(&entry.name, &[]);
        (media.to_owned(), crate::media::is_text_by_name(&entry.name))
    };
    FileStat {
        entry,
        hash,
        media_type,
        is_text,
    }
}

/// The time of last change of an entry in milliseconds, as a listing shows it.
#[must_use]
pub fn millis_of(mtime_ns: i64) -> i64 {
    mtime_ns / NANOS_PER_MILLI
}

/// The most entries a listing of a directory holds; what is beyond is cut and said.
pub fn cut(entries: &mut Vec<DirEntry>) -> bool {
    if entries.len() > LISTING_MAX_ENTRIES {
        entries.truncate(LISTING_MAX_ENTRIES);
        true
    } else {
        false
    }
}
