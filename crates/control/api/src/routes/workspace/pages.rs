//! Directory continuation selectors and bounded metadata windows; none grants authority.

use super::{listing, live};
use crate::middleware::requests::{ApiError, ApiResult};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::workspace::{DirEntry, DirEntryKind, FileListSource};
use orch_db::queries::environments::workspace::EnvironmentRow;
use orch_link_contract::commands::{FS_LIST_MAX_ENTRIES, FsListPageResult};
use orch_types::{Hash, Id, Time};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BinaryHeap;

// The same two-MiB page and existing frame allowance as the native snapshot reader.
const ENTRY_BYTES: usize =
    2 * 1024 * 1024 - orch_realtime_contract::version::MAX_CLIENT_FRAME_BYTES;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Source {
    Live,
    Manifest { blob: Hash },
    Opened,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Binding {
    pub(crate) environment: Id,
    pub(crate) generation: i32,
    pub(crate) machine: Option<Id>,
    pub(crate) path: String,
    pub(crate) mount: Option<String>,
    pub(crate) source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Key {
    directory: bool,
    name: String,
}

impl Key {
    fn of(entry: &DirEntry) -> Self {
        Self {
            directory: entry.kind == DirEntryKind::Dir,
            name: entry.name.clone(),
        }
    }
}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        (!self.directory)
            .cmp(&(!other.directory))
            .then_with(|| self.name.to_lowercase().cmp(&other.name.to_lowercase()))
            .then_with(|| self.name.cmp(&other.name))
    }
}
impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Position {
    Native { cursor: String },
    Cached { after: Key },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Cursor {
    binding: Hash,
    pub(crate) revision: String,
    pub(crate) position: Position,
    pub(crate) as_of: Time,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) observed_from: Option<Time>,
    expires_at: Time,
}

pub(crate) struct Metadata {
    pub(crate) revision: String,
    pub(crate) next_cursor: Option<String>,
    pub(crate) as_of: Time,
    pub(crate) observed_from: Option<Time>,
    pub(crate) expires_at: Time,
}

pub(crate) fn stale() -> ApiError {
    ApiError::new(
        ErrorCode::ConflictStaleRevision,
        "The directory listing changed or its continuation expired. Start this directory again.",
    )
    .with("restart", true)
    .with("reason", "directory_cursor")
}

pub(crate) fn resolve(
    env: &EnvironmentRow,
    machine: Option<Id>,
    source: FileListSource,
    mount: bool,
) -> ApiResult<Source> {
    if mount {
        return machine.map(|_| Source::Live).ok_or_else(live::offline);
    }
    if source == FileListSource::Live {
        return machine.map(|_| Source::Live).ok_or_else(live::offline);
    }
    if source == FileListSource::Auto && machine.is_some() {
        return Ok(Source::Live);
    }
    if !listing::indexed(env) {
        if machine.is_some() {
            return Err(ApiError::new(
                ErrorCode::WorkspaceNotIndexed,
                "the tree is too large to keep a list of; it is listed live",
            ));
        }
        return Ok(Source::Opened);
    }
    match (env.manifest_blob, env.manifest_at) {
        (Some(blob), Some(_)) => Ok(Source::Manifest { blob }),
        _ if machine.is_some() => Err(ApiError::new(
            ErrorCode::EnvironmentUnavailable,
            "the machine has not uploaded a list of the tree yet",
        )),
        _ => Err(live::offline()),
    }
}

impl Binding {
    fn hash(&self) -> ApiResult<Hash> {
        serde_json::to_vec(self)
            .map(|bytes| Hash::of(&bytes))
            .map_err(ApiError::internal)
    }

    pub(crate) fn cursor(&self, text: Option<&str>, now: Time) -> ApiResult<Option<Cursor>> {
        text.map(|text| {
            if text.len() > orch_realtime_contract::version::MAX_CLIENT_FRAME_BYTES {
                return Err(stale());
            }
            let bytes = URL_SAFE_NO_PAD.decode(text).map_err(|_| stale())?;
            let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| stale())?;
            if cursor.binding != self.hash()? || cursor.expires_at <= now {
                return Err(stale());
            }
            Ok(cursor)
        })
        .transpose()
    }

    pub(crate) fn agrees(&self, other: &Self) -> ApiResult<bool> {
        Ok(self.hash()? == other.hash()?)
    }

    pub(crate) fn metadata(
        &self,
        revision: String,
        position: Option<Position>,
        as_of: Time,
        now: Time,
    ) -> ApiResult<Metadata> {
        let expires_at = Time::new(now.get() + chrono::Duration::minutes(5));
        self.metadata_until(revision, position, as_of, None, expires_at)
    }

    fn metadata_until(
        &self,
        revision: String,
        position: Option<Position>,
        as_of: Time,
        observed_from: Option<Time>,
        expires_at: Time,
    ) -> ApiResult<Metadata> {
        let next_cursor = position
            .map(|position| {
                serde_json::to_vec(&Cursor {
                    binding: self.hash()?,
                    revision: revision.clone(),
                    position,
                    as_of,
                    observed_from,
                    expires_at,
                })
                .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
                .map_err(ApiError::internal)
            })
            .transpose()?;
        Ok(Metadata {
            revision,
            next_cursor,
            as_of,
            observed_from,
            expires_at,
        })
    }

    pub(crate) fn native(&self, page: FsListPageResult, now: Time) -> ApiResult<Metadata> {
        if page.observed_from.is_some_and(|start| start > page.as_of) {
            return Err(ApiError::new(
                ErrorCode::EnvironmentUnavailable,
                "The connector answered an invalid directory observation interval.",
            ));
        }
        let expires_at = page
            .expires_at
            .min(Time::new(now.get() + chrono::Duration::minutes(5)));
        self.metadata_until(
            format!("live:{}", page.revision),
            page.next_cursor.map(|cursor| Position::Native { cursor }),
            page.as_of,
            page.observed_from,
            expires_at,
        )
    }
}

struct Candidate {
    key: Key,
    entry: DirEntry,
    bytes: usize,
}
impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
impl Eq for Candidate {}
impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// At most one response window is retained, even when the cached manifest has millions of paths.
pub(crate) struct Window {
    after: Option<Key>,
    limit: usize,
    bytes: usize,
    rows: BinaryHeap<Candidate>,
    more: bool,
    frontier: Option<Key>,
}

impl Window {
    pub(crate) fn new(cursor: Option<&Cursor>, limit: u32) -> ApiResult<Self> {
        if limit == 0 {
            return Err(ApiError::invalid("limit", "must be at least 1"));
        }
        let after = cursor
            .map(|cursor| match &cursor.position {
                Position::Cached { after } => Ok(after.clone()),
                _ => Err(stale()),
            })
            .transpose()?;
        Ok(Self {
            after,
            limit: limit.min(FS_LIST_MAX_ENTRIES) as usize,
            bytes: 1,
            rows: BinaryHeap::new(),
            more: false,
            frontier: None,
        })
    }

    pub(crate) fn push(&mut self, entry: DirEntry) -> ApiResult<()> {
        let key = Key::of(&entry);
        if self.after.as_ref().is_some_and(|after| key <= *after) {
            return Ok(());
        }
        // Dropping a large row is a page boundary: admitting a later small row would
        // move the next cursor past that omitted sibling permanently.
        if self
            .frontier
            .as_ref()
            .is_some_and(|frontier| key >= *frontier)
        {
            self.more = true;
            return Ok(());
        }
        let bytes = serde_json::to_vec(&entry)
            .map_err(ApiError::internal)?
            .len()
            + 1;
        if bytes + 1 > ENTRY_BYTES {
            return Err(ApiError::new(
                ErrorCode::RequestTooLarge,
                "One directory entry exceeds the page size.",
            ));
        }
        self.bytes += bytes;
        self.rows.push(Candidate { key, entry, bytes });
        while self.rows.len() > self.limit || self.bytes > ENTRY_BYTES {
            if let Some(removed) = self.rows.pop() {
                self.bytes -= removed.bytes;
                self.more = true;
                if self
                    .frontier
                    .as_ref()
                    .is_none_or(|frontier| removed.key < *frontier)
                {
                    self.frontier = Some(removed.key);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn finish(self) -> (Vec<DirEntry>, Option<Position>) {
        let rows: Vec<_> = self
            .rows
            .into_sorted_vec()
            .into_iter()
            .map(|row| row.entry)
            .collect();
        let next = if self.more {
            rows.last().map(|entry| Position::Cached {
                after: Key::of(entry),
            })
        } else {
            None
        };
        (rows, next)
    }
}

pub(crate) fn manifest(
    tree: &orch_engine::manifest::Manifest,
    directory: &str,
    window: &mut Window,
) -> ApiResult<()> {
    if !directory.is_empty() {
        match tree.get(directory) {
            Some(entry) if entry.kind != orch_engine::manifest::EntryKind::Dir => {
                return Err(ApiError::not_found());
            }
            None if !tree.has_below(directory) => return Err(ApiError::not_found()),
            _ => {}
        }
    }
    let prefix = if directory.is_empty() {
        String::new()
    } else {
        format!("{directory}/")
    };
    let start = tree
        .entries()
        .partition_point(|entry| entry.path.as_str() < prefix.as_str());
    let mut previous: Option<&str> = None;
    for entry in &tree.entries()[start..] {
        let Some(rest) = entry.path.strip_prefix(&prefix) else {
            break;
        };
        if rest.is_empty() {
            continue;
        }
        let name = rest.split('/').next().ok_or_else(ApiError::not_found)?;
        if previous == Some(name) {
            continue;
        }
        previous = Some(name);
        let child = format!("{prefix}{name}");
        if let Some(named) = tree.get(&child) {
            window.push(listing::dir_entry(named))?;
        } else {
            window.push(DirEntry {
                name: name.to_owned(),
                path: child,
                kind: DirEntryKind::Dir,
                size: 0,
                mtime: Time::new(chrono::DateTime::from_timestamp_nanos(entry.mtime_ns)),
                mode: 0o755,
                link_target: None,
                cached: false,
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

    fn at() -> Proof<Time> {
        Ok(Time::new(
            chrono::DateTime::from_timestamp(1_760_000_000, 0).ok_or("clock")?,
        ))
    }
    fn binding() -> Binding {
        Binding {
            environment: orch_types::new_id(),
            generation: 1,
            machine: None,
            path: "web".to_owned(),
            mount: None,
            source: Source::Opened,
        }
    }
    fn entry(name: &str, size: usize, at: Time) -> DirEntry {
        DirEntry {
            name: name.to_owned(),
            path: format!("web/{name}"),
            kind: DirEntryKind::Symlink,
            size: 1,
            mtime: at,
            mode: 0o777,
            link_target: Some("x".repeat(size)),
            cached: false,
        }
    }

    #[test]
    fn bounded_unsorted_selection_never_skips_a_large_omitted_sibling() -> Proof {
        let binding = binding();
        let now = at()?;
        let mut cursor = None;
        let mut names = Vec::new();
        loop {
            let decoded = binding.cursor(cursor.as_deref(), now)?;
            let mut window = Window::new(decoded.as_ref(), 5000)?;
            for (name, size) in [("d", 10), ("c", 800_000), ("b", 800_000), ("a", 800_000)] {
                window.push(entry(name, size, now))?;
            }
            let (entries, next) = window.finish();
            assert!(!entries.is_empty());
            assert!(serde_json::to_vec(&entries)?.len() <= ENTRY_BYTES);
            names.extend(entries.into_iter().map(|entry| entry.name));
            let metadata = binding.metadata("frozen".to_owned(), next, now, now)?;
            cursor = metadata.next_cursor;
            if cursor.is_none() {
                break;
            }
            assert!(names.len() <= 4);
        }
        assert_eq!(names, ["a", "b", "c", "d"]);
        Ok(())
    }

    #[test]
    fn cursor_binding_and_expiry_fail_with_an_explicit_restart() -> Proof {
        let binding = binding();
        let now = at()?;
        let metadata = binding.metadata(
            "frozen".to_owned(),
            Some(Position::Cached {
                after: Key {
                    directory: false,
                    name: "a".to_owned(),
                },
            }),
            now,
            now,
        )?;
        let cursor = metadata.next_cursor.as_deref().ok_or("cursor")?;
        assert!(binding.cursor(Some(cursor), now)?.is_some());
        for other in [
            Binding {
                environment: orch_types::new_id(),
                ..binding.clone()
            },
            Binding {
                generation: 2,
                ..binding.clone()
            },
            Binding {
                path: "other".to_owned(),
                ..binding.clone()
            },
            Binding {
                mount: Some("assets".to_owned()),
                ..binding.clone()
            },
            Binding {
                source: Source::Live,
                ..binding.clone()
            },
            Binding {
                machine: Some(orch_types::new_id()),
                ..binding.clone()
            },
        ] {
            let problem = other
                .cursor(Some(cursor), now)
                .err()
                .ok_or("binding accepted")?;
            assert_eq!(problem.code(), ErrorCode::ConflictStaleRevision);
            assert_eq!(
                problem.problem.extra.get("restart"),
                Some(&serde_json::json!(true))
            );
        }
        assert!(binding.cursor(Some(cursor), metadata.expires_at).is_err());
        assert!(Window::new(None, 0).is_err());
        Ok(())
    }

    #[test]
    fn native_observation_interval_is_bound_into_the_http_cursor_and_reversed_intervals_are_refused()
    -> Proof {
        let binding = Binding {
            source: Source::Live,
            ..binding()
        };
        let start = at()?;
        let end = Time::new(start.get() + chrono::Duration::seconds(2));
        let page = FsListPageResult {
            revision: Hash::of(b"observed rows"),
            next_cursor: Some("native-second".to_owned()),
            as_of: end,
            observed_from: Some(start),
            expires_at: Time::new(end.get() + chrono::Duration::minutes(5)),
        };
        let metadata = binding.native(page.clone(), end)?;
        assert_eq!((metadata.observed_from, metadata.as_of), (Some(start), end));
        let cursor = binding
            .cursor(metadata.next_cursor.as_deref(), end)?
            .ok_or("HTTP continuation")?;
        assert_eq!((cursor.observed_from, cursor.as_of), (Some(start), end));
        let invalid = FsListPageResult {
            observed_from: Some(Time::new(end.get() + chrono::Duration::seconds(1))),
            ..page
        };
        let problem = binding
            .native(invalid, end)
            .err()
            .ok_or("a reversed interval was accepted")?;
        assert_eq!(problem.code(), ErrorCode::EnvironmentUnavailable);
        Ok(())
    }
}
