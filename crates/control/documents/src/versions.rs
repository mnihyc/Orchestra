//! Earlier versions: cut by the clock while a document is written and after it was left alone, before a rewrite and before one is put back; what lies between two is one entry with its authors; listed, read, put back, and kept within a number and a size.
//!
//! A version is the document whole at a moment. Nobody's writing is
//! recorded letter by letter: between two versions the document keeps a
//! tally of what was changed and by whom ([`Span`]), which the next version
//! takes as its authors and its counts, so that an afternoon of writing is a
//! handful of entries. The rules that say when to cut one are plain
//! functions of the clock: [`cut_before_a_batch`] for the batch that arrives,
//! [`cut_when_left_alone`] for the job that looks for documents nobody has
//! touched, and the cut before a version is put back is the caller's own.
//! Who wrote makes no difference: a person and an agent are alike in the
//! tally and in the rules.
//!
//! A history does not grow without end: [`dropped`] says which of the oldest
//! versions go when there are more than are kept or they come to more than
//! they may, and never the newest. A version may be held as the change from
//! an older one ([`Held`]); when the one it needs goes, it is held whole
//! again, and [`dropped_held`] counts it at that size.

use orch_api_contract::documents::{DocumentFileChange, VersionCause, VersionChanged};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::{BTreeSet, HashMap};

/// How many blocks a span of changes inserted, updated, moved and removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    /// Blocks put in.
    pub inserted: i64,
    /// Blocks whose type, props or content changed.
    pub updated: i64,
    /// Blocks put elsewhere.
    pub moved: i64,
    /// Blocks taken out.
    pub removed: i64,
}

impl Tally {
    /// Whether nothing was changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inserted == 0 && self.updated == 0 && self.moved == 0 && self.removed == 0
    }

    /// The tally as the API gives it.
    #[must_use]
    pub fn changed(&self) -> VersionChanged {
        VersionChanged {
            inserted: self.inserted,
            updated: self.updated,
            moved: self.moved,
            removed: self.removed,
            files: Vec::new(),
        }
    }
}

/// The distinct blocks inserted, updated, moved and removed by some writing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockChanges {
    /// Blocks put in.
    pub inserted: BTreeSet<Id>,
    /// Blocks whose type, props or content changed.
    pub updated: BTreeSet<Id>,
    /// Blocks put elsewhere.
    pub moved: BTreeSet<Id>,
    /// Blocks taken out.
    pub removed: BTreeSet<Id>,
}

impl BlockChanges {
    /// Counts each block once within each kind of change.
    #[must_use]
    pub fn tally(&self) -> Tally {
        let count = |ids: &BTreeSet<Id>| i64::try_from(ids.len()).unwrap_or(i64::MAX);
        Tally {
            inserted: count(&self.inserted),
            updated: count(&self.updated),
            moved: count(&self.moved),
            removed: count(&self.removed),
        }
    }

    fn to_json(&self) -> Json {
        json!({
            "inserted": self.inserted,
            "updated": self.updated,
            "moved": self.moved,
            "removed": self.removed,
        })
    }

    fn from_json(value: &Json) -> Self {
        let ids = |member: &str| {
            value
                .get(member)
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .filter_map(|value| value.as_str()?.parse().ok())
                .collect()
        };
        Self {
            inserted: ids("inserted"),
            updated: ids("updated"),
            moved: ids("moved"),
            removed: ids("removed"),
        }
    }
}

/// Who changed something: a person, or an agent that was let write, named by
/// the thread it worked in.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Writer {
    /// A person.
    User(Id),
    /// An agent, by its thread and its name then.
    Agent {
        /// The thread.
        thread: Id,
        /// The agent's name.
        agent: String,
    },
}

impl Writer {
    /// As a version stores it: `{ "user": id }` or `{ "thread": id, "agent": name }`.
    #[must_use]
    pub fn to_json(&self) -> Json {
        match self {
            Self::User(user) => json!({ "user": user }),
            Self::Agent { thread, agent } => json!({ "thread": thread, "agent": agent }),
        }
    }

    /// Reads what [`Writer::to_json`] wrote.
    #[must_use]
    pub fn from_json(value: &Json) -> Option<Self> {
        if let Some(user) = value.get("user").and_then(Json::as_str) {
            return Some(Self::User(user.parse().ok()?));
        }
        let thread = value.get("thread").and_then(Json::as_str)?.parse().ok()?;
        let agent = value.get("agent").and_then(Json::as_str)?.to_owned();
        Some(Self::Agent { thread, agent })
    }
}

/// What was changed since the last version was cut, and by whom.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Span {
    /// Who changed it, in the order they began.
    pub authors: Vec<Writer>,
    /// The first change.
    pub from_at: Option<Time>,
    /// The last change.
    pub to_at: Option<Time>,
    /// What was changed.
    pub tally: Tally,
    /// File names and their changes in this span, under the same authors.
    pub files: Vec<DocumentFileChange>,
    /// Identities are kept so repeated batches do not count a block again.
    blocks: BlockChanges,
}

impl Span {
    /// The counts and named file changes this span serves to a reader.
    #[must_use]
    pub fn changed(&self) -> VersionChanged {
        VersionChanged {
            files: self.files.clone(),
            ..self.tally.changed()
        }
    }

    /// Records a file under the writer's thread or person, including a file-only stretch of work.
    pub fn record_file(&mut self, writer: &Writer, file: DocumentFileChange, at: Time) {
        self.record(writer, &BlockChanges::default(), at);
        if !self.files.contains(&file) {
            self.files.push(file);
        }
    }

    /// Whether nothing was changed since the last version.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.from_at.is_none()
    }

    /// Takes in a batch that `writer` wrote at `at`, with the blocks it changed.
    pub fn record(&mut self, writer: &Writer, blocks: &BlockChanges, at: Time) {
        if !self.authors.contains(writer) {
            self.authors.push(writer.clone());
        }
        self.from_at = Some(self.from_at.map_or(at, |first| first.min(at)));
        self.to_at = Some(self.to_at.map_or(at, |last| last.max(at)));
        let before = self.blocks.tally();
        self.blocks.inserted.extend(&blocks.inserted);
        self.blocks.updated.extend(&blocks.updated);
        self.blocks.moved.extend(&blocks.moved);
        self.blocks.removed.extend(&blocks.removed);
        let after = self.blocks.tally();
        // Older spans have counts but no identities. Keep those recorded facts
        // until the next cut; writing tracked here counts each new id once.
        self.tally.inserted = self
            .tally
            .inserted
            .saturating_add(after.inserted - before.inserted);
        self.tally.updated = self
            .tally
            .updated
            .saturating_add(after.updated - before.updated);
        self.tally.moved = self.tally.moved.saturating_add(after.moved - before.moved);
        self.tally.removed = self
            .tally
            .removed
            .saturating_add(after.removed - before.removed);
    }

    /// As the document row keeps it.
    #[must_use]
    pub fn to_json(&self) -> Json {
        let (Some(from), Some(to)) = (self.from_at, self.to_at) else {
            return json!({});
        };
        json!({
            "authors": self.authors.iter().map(Writer::to_json).collect::<Vec<_>>(),
            "from_at": from.to_string(),
            "to_at": to.to_string(),
            "inserted": self.tally.inserted,
            "updated": self.tally.updated,
            "moved": self.tally.moved,
            "removed": self.tally.removed,
            "blocks": self.blocks.to_json(),
            "files": self.files,
        })
    }

    /// Reads what [`Span::to_json`] wrote; an empty object is an empty span.
    #[must_use]
    pub fn from_json(value: &Json) -> Self {
        let moment = |member: &str| {
            value
                .get(member)
                .and_then(Json::as_str)
                .and_then(|text| text.parse::<Time>().ok())
        };
        let number = |member: &str| value.get(member).and_then(Json::as_i64).unwrap_or(0);
        let (Some(from_at), Some(to_at)) = (moment("from_at"), moment("to_at")) else {
            return Self::default();
        };
        Self {
            authors: value
                .get("authors")
                .and_then(Json::as_array)
                .map(|authors| authors.iter().filter_map(Writer::from_json).collect())
                .unwrap_or_default(),
            from_at: Some(from_at),
            to_at: Some(to_at),
            tally: Tally {
                inserted: number("inserted"),
                updated: number("updated"),
                moved: number("moved"),
                removed: number("removed"),
            },
            blocks: BlockChanges::from_json(&value["blocks"]),
            files: serde_json::from_value(value.get("files").cloned().unwrap_or_else(|| json!([])))
                .unwrap_or_default(),
        }
    }
}

fn seconds_between(earlier: Time, later: Time) -> i64 {
    (later.unix_millis() - earlier.unix_millis()).div_euclid(1_000)
}

/// Whether to cut a version of the document as it stands, before a batch
/// that arrives at `now` is applied, and why.
///
/// Only a document that changed since its last version has anything to cut.
/// Then: a batch that is about to replace everything cuts one first
/// ([`VersionCause::Rewrite`]), and so does one that arrives when the last
/// version was cut longer ago than `window_secs` ([`VersionCause::Window`]);
/// a document that has none yet counts from the moment it was made.
#[must_use]
pub fn cut_before_a_batch(
    span: &Span,
    last_cut: Option<Time>,
    created_at: Time,
    now: Time,
    window_secs: i64,
    replaces_everything: bool,
) -> Option<VersionCause> {
    if span.is_empty() {
        return None;
    }
    if replaces_everything {
        return Some(VersionCause::Rewrite);
    }
    let since = last_cut.unwrap_or(created_at);
    (seconds_between(since, now) > window_secs).then_some(VersionCause::Window)
}

/// Whether a document that changed since its last version, and that nobody
/// has written in for `idle_secs`, is to have one cut now, so that what
/// somebody leaves behind is itself a version.
#[must_use]
pub fn cut_when_left_alone(span: &Span, now: Time, idle_secs: i64) -> bool {
    span.to_at
        .is_some_and(|last| seconds_between(last, now) >= idle_secs)
}

/// Which versions go when a history holds more than it may. `kept` are the
/// versions of the document as `(number, bytes)`, newest first, each held
/// whole. The oldest go first, until at most `max_versions` stay and they come
/// to at most `max_bytes` together; the newest never goes, however large it is.
#[must_use]
pub fn dropped(versions: &[(i64, i64)], max_versions: i64, max_bytes: i64) -> Vec<i64> {
    let held: Vec<Held> = versions
        .iter()
        .map(|&(number, bytes)| Held {
            number,
            stored: bytes,
            whole: bytes,
            base: None,
        })
        .collect();
    dropped_held(&held, max_versions, max_bytes)
}

/// A version as its history holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    /// Its number.
    pub number: i64,
    /// The size of what is stored for it: its blocks whole, or its change.
    pub stored: i64,
    /// The size of its blocks whole.
    pub whole: i64,
    /// The older version it is the change from, when it is held as one.
    pub base: Option<i64>,
}

/// Which versions go when a history holds more than it may. `versions` are
/// newest first. The history keeps the newest versions and lets the older ones
/// go, as many as possible within `max_versions` and `max_bytes`, and never the
/// newest. A kept version whose base goes is held whole again and counts its
/// whole size; when the base stays, it counts the change.
#[must_use]
pub fn dropped_held(versions: &[Held], max_versions: i64, max_bytes: i64) -> Vec<i64> {
    let mut cost = 0_i64;
    // For each base not kept yet: how much less the kept versions that need it come to once it is kept.
    let mut relief: HashMap<i64, i64> = HashMap::new();
    let mut keep = 0_usize;
    for (index, held) in versions.iter().enumerate() {
        let count = i64::try_from(index + 1).unwrap_or(i64::MAX);
        if index > 0 && count > max_versions {
            break;
        }
        let own = if held.base.is_some() {
            held.whole
        } else {
            held.stored
        };
        cost = cost
            .saturating_add(own)
            .saturating_sub(relief.remove(&held.number).unwrap_or(0));
        if let Some(base) = held.base {
            let saved = held.whole.saturating_sub(held.stored);
            let entry = relief.entry(base).or_insert(0);
            *entry = entry.saturating_add(saved);
        }
        if index == 0 || cost <= max_bytes {
            keep = index + 1;
        }
    }
    versions[keep..].iter().map(|held| held.number).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    fn at(second: i64) -> Time {
        let base: Time = "2026-10-08T17:00:00.000Z".parse().unwrap();
        Time::new(base.get() + chrono::Duration::seconds(second))
    }

    fn changed_span(from: i64, to: i64) -> Span {
        let mut span = Span::default();
        span.record(
            &Writer::User(new_id()),
            &BlockChanges {
                updated: BTreeSet::from([new_id()]),
                ..BlockChanges::default()
            },
            at(from),
        );
        span.record(
            &Writer::User(new_id()),
            &BlockChanges {
                inserted: BTreeSet::from([new_id(), new_id()]),
                ..BlockChanges::default()
            },
            at(to),
        );
        span
    }

    #[test]
    fn what_was_changed_between_two_versions_is_one_tally_with_every_author_once() {
        let (ada, bo) = (Writer::User(new_id()), Writer::User(new_id()));
        let agent = Writer::Agent {
            thread: new_id(),
            agent: "codex".into(),
        };
        let mut span = Span::default();
        assert!(span.is_empty());
        let (a, b, c, d, e, f) = (new_id(), new_id(), new_id(), new_id(), new_id(), new_id());
        let one = |inserted: &[Id], updated: &[Id], moved: &[Id], removed: &[Id]| BlockChanges {
            inserted: inserted.iter().copied().collect(),
            updated: updated.iter().copied().collect(),
            moved: moved.iter().copied().collect(),
            removed: removed.iter().copied().collect(),
        };
        span.record(&ada, &one(&[a, b], &[], &[], &[]), at(5));
        span.record(&bo, &one(&[], &[c, d, e], &[c], &[]), at(9));
        span.record(&ada, &one(&[], &[c], &[], &[a, b, c, d]), at(7));
        span.record(&agent, &one(&[f], &[], &[], &[]), at(12));
        assert_eq!(span.authors, [ada, bo, agent], "in the order they began");
        assert_eq!(
            span.tally,
            Tally {
                inserted: 3,
                updated: 3,
                moved: 1,
                removed: 4
            }
        );
        assert_eq!((span.from_at, span.to_at), (Some(at(5)), Some(at(12))));
        let kept = Span::from_json(&span.to_json());
        assert_eq!(kept, span, "the document row keeps it and reads it back");
        assert!(Span::from_json(&json!({})).is_empty());
        assert_eq!(Span::default().to_json(), json!({}));
    }

    #[test]
    fn repeated_changes_count_each_block_once_in_each_category_after_the_span_is_loaded_again() {
        let writer = Writer::User(new_id());
        let (a, b) = (new_id(), new_id());
        let changes = BlockChanges {
            inserted: BTreeSet::from([a, b]),
            updated: BTreeSet::from([a]),
            moved: BTreeSet::from([b]),
            removed: BTreeSet::from([a, b]),
        };
        let mut span = Span::default();
        span.record(&writer, &changes, at(1));
        let mut loaded = Span::from_json(&span.to_json());
        loaded.record(&writer, &changes, at(2));
        assert_eq!(loaded.tally, changes.tally());
        loaded.record(
            &writer,
            &BlockChanges {
                updated: BTreeSet::from([a, b]),
                ..BlockChanges::default()
            },
            at(3),
        );
        assert_eq!(
            loaded.tally,
            Tally {
                inserted: 2,
                updated: 2,
                moved: 1,
                removed: 2
            }
        );
        assert_eq!(loaded.authors, [writer]);
        assert_eq!((loaded.from_at, loaded.to_at), (Some(at(1)), Some(at(3))));
    }

    #[test]
    fn file_only_work_keeps_names_and_thread_authorship_through_storage_and_a_window_cut() {
        use orch_api_contract::documents::DocumentFileChangeKind;
        let agent = Writer::Agent {
            thread: new_id(),
            agent: "Writer".to_owned(),
        };
        let person = Writer::User(new_id());
        let change = |kind| DocumentFileChange {
            name: "report.csv".to_owned(),
            kind,
        };
        let mut span = Span::default();
        span.record_file(&agent, change(DocumentFileChangeKind::Added), at(5));
        let mut loaded = Span::from_json(&span.to_json());
        loaded.record_file(&agent, change(DocumentFileChangeKind::Added), at(7));
        loaded.record_file(&agent, change(DocumentFileChangeKind::Replaced), at(9));
        loaded.record_file(&person, change(DocumentFileChangeKind::Removed), at(10));
        assert!(!loaded.is_empty(), "file-only work belongs in the history");
        assert!(loaded.tally.is_empty(), "no block counts are invented");
        assert_eq!(loaded.authors, [agent, person]);
        assert_eq!((loaded.from_at, loaded.to_at), (Some(at(5)), Some(at(10))));
        assert_eq!(
            loaded.files,
            [
                change(DocumentFileChangeKind::Added),
                change(DocumentFileChangeKind::Replaced),
                change(DocumentFileChangeKind::Removed)
            ]
        );
        assert_eq!(loaded.changed().files, loaded.files);
        assert_eq!(Span::from_json(&loaded.to_json()), loaded);
        assert_eq!(
            cut_before_a_batch(&loaded, None, at(0), at(600), 600, false),
            None
        );
        assert_eq!(
            cut_before_a_batch(&loaded, None, at(0), at(601), 600, false),
            Some(VersionCause::Window)
        );
        assert!(cut_when_left_alone(&loaded, at(130), 120));
        let mut old = loaded.to_json();
        old.as_object_mut().unwrap().remove("files");
        let old = Span::from_json(&old);
        assert!(old.files.is_empty(), "older text-only spans still read");
        assert_eq!(old.authors, loaded.authors);
        assert_eq!(old.tally, loaded.tally);
    }

    #[test]
    fn an_older_count_only_span_keeps_its_totals_and_deduplicates_new_writing() {
        let writer = Writer::User(new_id());
        let mut span = Span::from_json(&json!({
            "authors": [writer.to_json()],
            "from_at": at(1).to_string(),
            "to_at": at(2).to_string(),
            "inserted": 3,
            "updated": 9,
        }));
        let changes = BlockChanges {
            updated: BTreeSet::from([new_id()]),
            ..BlockChanges::default()
        };
        span.record(&writer, &changes, at(3));
        let mut loaded = Span::from_json(&span.to_json());
        loaded.record(&writer, &changes, at(4));
        assert_eq!(
            loaded.tally,
            Tally {
                inserted: 3,
                updated: 10,
                moved: 0,
                removed: 0
            }
        );
        assert_eq!(loaded.from_at, Some(at(1)));
        assert_eq!(loaded.authors, [writer]);
    }

    #[test]
    fn a_batch_cuts_a_version_only_when_the_last_was_cut_longer_ago_than_the_window() {
        let span = changed_span(0, 30);
        let cut = |now| cut_before_a_batch(&span, Some(at(0)), at(-5000), at(now), 600, false);
        assert_eq!(cut(600), None, "exactly the window is not longer");
        assert_eq!(cut(601), Some(VersionCause::Window));
        assert_eq!(cut(10), None);
    }

    #[test]
    fn a_document_without_a_version_counts_its_window_from_the_moment_it_was_made() {
        let span = changed_span(0, 30);
        let cut = |now| cut_before_a_batch(&span, None, at(0), at(now), 600, false);
        assert_eq!(cut(500), None);
        assert_eq!(cut(700), Some(VersionCause::Window));
    }

    #[test]
    fn a_batch_that_replaces_everything_cuts_one_first_whatever_the_clock_says() {
        let span = changed_span(0, 30);
        assert_eq!(
            cut_before_a_batch(&span, Some(at(0)), at(0), at(31), 600, true),
            Some(VersionCause::Rewrite)
        );
    }

    #[test]
    fn nothing_is_cut_for_a_document_that_has_not_changed_since_its_last_version() {
        let empty = Span::default();
        assert_eq!(
            cut_before_a_batch(&empty, Some(at(0)), at(0), at(9_999), 600, true),
            None
        );
        assert!(!cut_when_left_alone(&empty, at(9_999), 120));
    }

    #[test]
    fn a_document_left_alone_for_the_idle_time_is_cut() {
        let span = changed_span(0, 30);
        assert!(!cut_when_left_alone(&span, at(30 + 119), 120));
        assert!(cut_when_left_alone(&span, at(30 + 120), 120));
    }

    #[test]
    fn the_oldest_versions_go_first_when_there_are_too_many_or_they_are_too_large() {
        // newest first
        let versions = [(5, 10), (4, 10), (3, 10), (2, 10), (1, 10)];
        assert_eq!(dropped(&versions, 5, 1_000), Vec::<i64>::new());
        assert_eq!(dropped(&versions, 3, 1_000), [2, 1]);
        assert_eq!(dropped(&versions, 99, 35), [2, 1], "three fit in 35 bytes");
        assert_eq!(dropped(&versions, 99, 30), [2, 1]);
        assert_eq!(dropped(&versions, 99, 29), [3, 2, 1]);
    }

    #[test]
    fn the_newest_version_is_never_removed_however_large_it_is() {
        let versions = [(3, 500), (2, 10), (1, 10)];
        assert_eq!(dropped(&versions, 200, 100), [2, 1]);
        assert_eq!(dropped(&versions, 0, 100), [2, 1]);
        assert_eq!(dropped(&[(1, 9_999)], 1, 1), Vec::<i64>::new());
    }

    #[test]
    fn once_one_version_is_dropped_every_older_one_goes_with_it() {
        // A small old version does not survive a large newer one that was dropped.
        let versions = [(4, 10), (3, 90), (2, 1), (1, 1)];
        assert_eq!(dropped(&versions, 99, 50), [3, 2, 1]);
    }

    #[test]
    fn a_change_counts_its_own_size_while_its_base_stays_and_its_whole_size_once_the_base_goes() {
        let change = |number, base| Held {
            number,
            stored: 5,
            whole: 100,
            base: Some(base),
        };
        let whole = |number| Held {
            number,
            stored: 100,
            whole: 100,
            base: None,
        };
        // newest first: 4 and 3 are changes on 2, which is a change on 1, held whole
        let versions = [change(4, 3), change(3, 2), change(2, 1), whole(1)];
        assert_eq!(
            dropped_held(&versions, 99, 115),
            Vec::<i64>::new(),
            "5 + 5 + 5 + 100"
        );
        assert_eq!(
            dropped_held(&versions, 3, 1_000),
            [1],
            "2 is held whole again: 5 + 5 + 100"
        );
        assert_eq!(
            dropped_held(&versions, 99, 114),
            [1],
            "5 + 5 + 100 fits, 115 does not"
        );
        assert_eq!(
            dropped_held(&versions, 99, 104),
            [3, 2, 1],
            "only the newest, held whole, fits"
        );
        assert_eq!(
            dropped_held(&versions, 99, 99),
            [3, 2, 1],
            "the newest stays however large"
        );
        // keeping the base can make the kept versions smaller than without it
        let small = Held {
            number: 1,
            stored: 40,
            whole: 40,
            base: None,
        };
        let shrunk = [change(3, 2), change(2, 1), small];
        assert_eq!(
            dropped_held(&shrunk, 99, 60),
            Vec::<i64>::new(),
            "5 + 5 + 40 fits, 5 + 100 does not"
        );
    }
}
