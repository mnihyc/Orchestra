//! Bringing a thread as history: the thread node under its topic, with its turns and items as the package records them, read-only, and the taking back of what an import added to a thread that was there.
//!
//! A thread comes as what was said and done in it, and as nothing that could run: its node lies under the topic in the state
//! `blocked` for the reason `unbound`, so that nobody mistakes it for a conversation that goes on until it is given an agent and an
//! environment. Its items are written under one segment of their own that ran on nothing here (the NULL binding stands for its agent
//! profile, provider and environment, and its report names what it ran on where it was made), marked as history from before Orch:
//! in no turn, read-only, never sent to an agent again. The turns of the source are not made again, since a turn is a promise about
//! what a harness did, which nothing here can keep; the report says how many there were.
//!
//! The record is read through one map. Who said a thing is the person it is written as (the account they are taken for, where the
//! importer may write as them; otherwise the importer, with "Originally by" beside the words). A file it names is a file the package
//! carried or one the thread holds already, and is otherwise said not to have come: a package may not make the store's other files
//! readable through a thread by naming their hashes. What a source holds about itself that does not apply here (the lines about what
//! happened to the thread there, the requests for approval, the identifiers of inputs) is left out and counted. An item has the
//! identifier derived from the thread it joins and its own in the source, so that the same item brought twice is one item, and a
//! later package of the same conversation adds only what is not there yet.

use super::{Batch, Made, MakeError, Record, Taken, free_name, note_origin, refuse};
use crate::imports::identity;
use crate::imports::model::Object;
use crate::imports::people::Credits;
use crate::imports::records::{self, Cut, Piece};
use crate::projections::ItemTexts;
use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::pages::StoredKind;
use orch_agent_contract::events::MAIN_STREAM;
use orch_api_contract::forks::{Fidelity, HistoryReport, ReportDropped, ReportKind};
use orch_api_contract::imports::{ImportAction, ImportOutcome, ImportWarning};
use orch_api_contract::package::PackageKind;
use orch_api_contract::threads::ItemPayload;
use orch_db::Tx;
use orch_db::audit::{Actor, Entry as AuditEntry, append};
use orch_db::queries::hierarchy;
use orch_db::queries::imports::{OriginRow, kind as origin_kind};
use orch_db::queries::imports_chats::place_is_live;
use orch_db::queries::imports_threads as rows;
use orch_db::queries::threads::{self, StateChange, thread_state};
use orch_types::{Hash, Id, Json, Seq, Time};
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use std::ops::Range;

/// The most characters of a thread's name: the limit of the column.
const NAME_MAX: usize = 120;
/// The most characters of a node's description: the limit of the column.
const DESCRIPTION_MAX: usize = 4000;
/// What a thread is called whose name the package could not give.
const UNNAMED_THREAD: &str = "Imported thread";
/// The reason a thread that nothing runs is blocked for, in the word the column has.
const UNBOUND: &str = "unbound";
/// What an imported thread says of why nothing runs it.
const UNBOUND_DETAIL: &str = "It was imported as history, so nothing runs it yet. Give it an agent and an environment to go on.";
/// The kind of row an item is, in the identifier it is given.
const ITEM_ROW: &str = "item";
/// The kind of row the segment of imported history is, in the identifier it is given.
const SEGMENT_ROW: &str = "segment";
/// The label that stands for an item or a thread whose kind the record did not say.
const UNKNOWN_KIND: &str = "item";
/// How many numbers of the thread's sequence an import takes besides one for each item: one for the thread's state and one for the
/// segment.
const SEQS_BESIDES_ITEMS: i64 = 2;
/// The most items one statement writes. A page keeps a statement to a size the server parses without strain, and a transaction's
/// memory to a few pages; it limits nothing about what a thread may hold.
const ITEMS_PER_STATEMENT: usize = 200;
/// The most bytes of payload one statement carries, for the same reason; an item that is larger than this stands in a statement of
/// its own.
const STATEMENT_BYTES_MAX: usize = 4 * 1024 * 1024;
/// The arrangement types, in the words of the column.
const ARRANGEMENTS: [&str; 5] = [
    "separated",
    "key_separated",
    "credential_shared",
    "handed_over",
    "together",
];
/// The arrangement a segment of imported history is written with where the source did not say one that is known.
const NEUTRAL_ARRANGEMENT: &str = "together";
/// The credential paths, in the words of the column.
const CREDENTIAL_PATHS: [&str; 4] = ["native", "forwarded", "shared", "handover"];
/// The credential path a segment of imported history is written with where the source did not say one that is known.
const NEUTRAL_CREDENTIAL_PATH: &str = "native";
/// The kinds of item the column holds.
const ITEM_KINDS: [&str; 9] = [
    "user_message",
    "agent_message",
    "reasoning",
    "plan",
    "tool_call",
    "approval",
    "announcement",
    "marker",
    "error",
];

/// Why a line about the source's own acts is left out.
const DROPPED_MARKER: &str = "lines about what happened to the thread where it was made (changes, switches, forks) do not apply here";
/// Why a request for approval is left out.
const DROPPED_APPROVAL: &str = "the request an approval stood for is not part of the package, so there is nothing to answer here";
/// Why an item that cannot be read is left out.
const DROPPED_UNREADABLE: &str = "it could not be read as an item of its kind";
/// What a report says of how the conversation was brought, whatever else it says.
const HISTORY_ONLY: &str = "The conversation is brought as history: its items stand in the order they were written, outside the turns they came in.";

// ---------------------------------------------------------------- reading the record

/// An item of the record, as far as it reads.
#[derive(Debug, Clone)]
struct Candidate {
    source: Id,
    kind: String,
    stream: String,
    status: String,
    payload: Json,
    parent: Option<Id>,
    turn: Option<Id>,
    author: Option<Id>,
    created_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
}

/// The identifier a member of a record is written as.
fn id_of(value: Option<&Json>) -> Option<Id> {
    value?.as_str()?.parse().ok()
}

/// The moment a member of a record is written as.
fn time_of(value: Option<&Json>) -> Option<DateTime<Utc>> {
    value?.as_str()?.parse::<Time>().ok().map(Time::get)
}

/// An item of the record as it reads, where it says what it is.
fn candidate(value: &Json) -> Option<Candidate> {
    Some(Candidate {
        source: id_of(value.get("id"))?,
        kind: value.get("kind")?.as_str()?.to_owned(),
        stream: value
            .get("stream")
            .and_then(Json::as_str)
            .unwrap_or(MAIN_STREAM)
            .to_owned(),
        status: value.get("status")?.as_str()?.to_owned(),
        payload: value.get("payload")?.clone(),
        parent: id_of(value.get("parent_item_id")),
        turn: id_of(value.get("turn_id")),
        author: id_of(value.get("author").and_then(|author| author.get("id"))),
        created_at: time_of(value.get("created_at")),
        updated_at: time_of(value.get("updated_at")),
    })
}

/// The items of a page of a record in the order the record has them, and how many entries of it do not read as items at all. The export
/// writes a conversation in the order it was made, so the order of the record is the order of the conversation.
fn candidates_in(page: &[Json]) -> (Vec<Candidate>, i64) {
    let mut unreadable = 0;
    let found: Vec<Candidate> = page
        .iter()
        .filter_map(|entry| {
            let read = candidate(entry);
            if read.is_none() {
                unreadable += 1;
            }
            read
        })
        .collect();
    (found, unreadable)
}

/// What becomes of an item of its status here: one that was under way when the source was written is not, and never will be, and a
/// request that waited is one nobody can answer.
fn status_here(word: &str) -> Option<&'static str> {
    match word {
        "in_progress" => Some("cancelled"),
        "pending" => Some("expired"),
        "completed" => Some("completed"),
        "failed" => Some("failed"),
        "cancelled" => Some("cancelled"),
        "approved" => Some("approved"),
        "answered" => Some("answered"),
        "denied" => Some("denied"),
        "expired" => Some("expired"),
        _ => None,
    }
}

/// Why an item is left out, where it is.
fn why_left_out(candidate: &Candidate) -> Option<&'static str> {
    if !ITEM_KINDS.contains(&candidate.kind.as_str()) || status_here(&candidate.status).is_none() {
        return Some(DROPPED_UNREADABLE);
    }
    match candidate.kind.as_str() {
        "marker" => Some(DROPPED_MARKER),
        "approval" => Some(DROPPED_APPROVAL),
        _ => None,
    }
}

// ---------------------------------------------------------------- the files an item names

/// The file a value names, where it is a hash.
fn blob_of(value: &Json) -> Option<Hash> {
    value.as_str()?.parse().ok()
}

/// Every file a payload names, whatever the shape it names it in: a `blob` member is a file by its hash.
fn blobs_in(value: &Json, found: &mut Vec<Hash>) {
    match value {
        Json::Array(list) => {
            for each in list {
                blobs_in(each, found);
            }
        }
        Json::Object(map) => {
            for (key, member) in map {
                match blob_of(member) {
                    Some(hash) if key == "blob" => found.push(hash),
                    _ => blobs_in(member, found),
                }
            }
        }
        _ => {}
    }
}

/// What the files that did not come cost an item.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Lost {
    /// References that were turned into words or taken out.
    replaced: i64,
    /// References that stay as they were, since what holds them cannot do without: the file will not open.
    dangling: i64,
}

/// Whether an object names a file by its `blob` member that did not come.
fn is_missing(map: &serde_json::Map<String, Json>, here: &HashSet<Hash>) -> bool {
    map.get("blob")
        .and_then(blob_of)
        .is_some_and(|hash| !here.contains(&hash))
}

/// What stands where a part that names a file that did not come stood.
enum StandIn {
    /// Words that say so.
    Part(Json),
    /// Nothing: an entry of a list that is a list of files.
    Nothing,
    /// What the part says is kept, without the file.
    Same,
}

/// What stands in a part of a payload whose file did not come.
fn stand_in(part: &Json) -> StandIn {
    let name = part.get("name").and_then(Json::as_str).unwrap_or("a file");
    let note =
        || json!({ "type": "text", "text": format!("The file \"{name}\" was not brought.") });
    match part.get("type").and_then(Json::as_str) {
        Some("image") => StandIn::Part(note()),
        Some("pasted_text") => match part.get("inline_text").and_then(Json::as_str) {
            Some(text) => StandIn::Part(json!({ "type": "text", "text": text })),
            None => StandIn::Part(note()),
        },
        None if part.get("name").is_some() => StandIn::Nothing,
        _ => StandIn::Same,
    }
}

/// Writes a payload so that it names no file that is not here: an image that did not come is words that say so, the words of a
/// folded paste are kept as they were said, a file that was sent keeps its name and size without the bytes, and the whole of a
/// long output that did not come is as if it had been cut.
fn degrade(value: &mut Json, here: &HashSet<Hash>, lost: &mut Lost) {
    match value {
        Json::Array(list) => {
            let mut at = 0;
            while at < list.len() {
                let missing = list[at]
                    .as_object()
                    .is_some_and(|map| is_missing(map, here));
                if missing {
                    match stand_in(&list[at]) {
                        StandIn::Part(part) => {
                            list[at] = part;
                            lost.replaced += 1;
                            at += 1;
                            continue;
                        }
                        StandIn::Nothing => {
                            list.remove(at);
                            lost.replaced += 1;
                            continue;
                        }
                        StandIn::Same => {}
                    }
                }
                degrade(&mut list[at], here, lost);
                at += 1;
            }
        }
        Json::Object(map) => {
            if is_missing(map, here) {
                // A file that a part may be without, and an output that may be cut, say so by being null.
                let optional = map.contains_key("inline")
                    || map.get("type").and_then(Json::as_str) == Some("file");
                if optional {
                    map.insert("blob".to_owned(), Json::Null);
                    lost.replaced += 1;
                } else {
                    lost.dangling += 1;
                }
            }
            for member in map.values_mut() {
                degrade(member, here, lost);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- an item, as it is written here

/// What an item is adapted with.
struct Adapt<'a> {
    thread: Id,
    here: &'a HashSet<Hash>,
    credits: &'a Credits,
    now: DateTime<Utc>,
}

/// An item as it is written here.
#[derive(Debug, Clone)]
struct Ready {
    id: Id,
    source: Id,
    parent: Option<Id>,
    turn: Option<Id>,
    stream: String,
    kind: String,
    status: &'static str,
    /// The account that said it here.
    author: Option<Id>,
    /// The person of the source who said it.
    by: Option<Id>,
    payload: Json,
    search_text: Option<String>,
    size: usize,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    /// Whether it says anything other than what it said.
    changed: bool,
}

/// The words that say who said a thing where it is not written as them.
fn originally_by(name: &str) -> String {
    format!("Originally by {name}")
}

/// Puts the words that say who said a message before what it said.
fn label(payload: &mut Json, name: &str) {
    if let Some(parts) = payload.get_mut("parts").and_then(Json::as_array_mut) {
        parts.insert(0, json!({ "type": "text", "text": originally_by(name) }));
    }
}

/// Whether a payload reads as one of its kind.
fn reads(kind: &str, payload: &Json) -> bool {
    serde_json::from_value::<ItemPayload>(json!({ "kind": kind, "payload": payload })).is_ok()
}

/// Writes an item of the record as it is written here, or says why it is left out.
fn adapt(candidate: Candidate, with: &Adapt<'_>, lost: &mut Lost) -> Result<Ready, &'static str> {
    let Candidate {
        source,
        kind,
        stream,
        status,
        mut payload,
        parent,
        turn,
        author,
        created_at,
        updated_at,
        ..
    } = candidate;
    let status_now = status_here(&status).ok_or(DROPPED_UNREADABLE)?;
    let mut changed = status_now != status;
    let before = *lost;
    degrade(&mut payload, with.here, lost);
    changed |= *lost != before;
    if let Some(map) = payload.as_object_mut() {
        match kind.as_str() {
            // The input it came in is not here, and neither is the approval of a plan.
            "user_message" => {
                map.insert("input_id".to_owned(), Json::Null);
            }
            "plan" => {
                map.remove("approval_id");
                map.remove("decision");
            }
            _ => {}
        }
    }
    let mut account = None;
    if kind == "user_message"
        && let Some(person) = author
    {
        let credit = with.credits.of(Some(person));
        account = Some(credit.user);
        if let Some(name) = credit.original {
            label(&mut payload, &name);
            changed = true;
        }
    }
    if !reads(&kind, &payload) {
        return Err(DROPPED_UNREADABLE);
    }
    let created = created_at.or(updated_at).unwrap_or(with.now);
    let updated = updated_at.unwrap_or(created).max(created);
    Ok(Ready {
        id: identity::inside(with.thread, ITEM_ROW, source),
        source,
        parent,
        turn,
        stream,
        kind,
        status: status_now,
        author: account,
        by: author,
        size: payload.to_string().len(),
        payload,
        search_text: None,
        created_at: created,
        updated_at: updated,
        changed,
    })
}

// ---------------------------------------------------------------- what it comes to

/// What became of the items of a record, counted as a report counts them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Tally {
    total: i64,
    kinds: BTreeMap<String, ReportKind>,
    dropped: BTreeMap<(String, &'static str), i64>,
    /// How many references to files that did not come were turned into words or taken out.
    files_replaced: i64,
    /// How many references to files that did not come stay as they were.
    files_dangling: i64,
}

impl Tally {
    fn of(&mut self, kind: &str) -> &mut ReportKind {
        self.kinds.entry(kind.to_owned()).or_insert(ReportKind {
            kept: 0,
            converted: 0,
            dropped: 0,
        })
    }

    fn kept(&mut self, kind: &str, changed: bool) {
        self.total += 1;
        let counts = self.of(kind);
        if changed {
            counts.converted += 1;
        } else {
            counts.kept += 1;
        }
    }

    fn left_out(&mut self, kind: &str, why: &'static str) {
        self.total += 1;
        self.of(kind).dropped += 1;
        *self.dropped.entry((kind.to_owned(), why)).or_default() += 1;
    }

    fn how_many_left_out(&self) -> i64 {
        self.dropped.values().sum()
    }

    fn files_lost(&self) -> i64 {
        self.files_replaced + self.files_dangling
    }

    fn into_report(self) -> HistoryReport {
        let mut warnings = vec![HISTORY_ONLY.to_owned()];
        if self.files_lost() > 0 {
            warnings.push(format!(
                "{} references to files that did not come were turned into words, taken out or left as they were.",
                self.files_lost()
            ));
        }
        HistoryReport {
            fidelity: Fidelity::Structured,
            items_total: self.total,
            by_kind: self.kinds,
            dropped: self
                .dropped
                .into_iter()
                .map(|((kind, reason), count)| ReportDropped {
                    kind,
                    count,
                    reason: reason.to_owned(),
                })
                .collect(),
            warnings,
        }
    }
}

/// What two reports of the same segment come to together: the counts add, and a warning that is said already is not said again.
fn merged(old: HistoryReport, new: HistoryReport) -> HistoryReport {
    let mut by_kind = old.by_kind;
    for (kind, add) in new.by_kind {
        let held = by_kind.entry(kind).or_insert(ReportKind {
            kept: 0,
            converted: 0,
            dropped: 0,
        });
        held.kept += add.kept;
        held.converted += add.converted;
        held.dropped += add.dropped;
    }
    let mut dropped = old.dropped;
    for add in new.dropped {
        match dropped
            .iter_mut()
            .find(|held| held.kind == add.kind && held.reason == add.reason)
        {
            Some(held) => held.count += add.count,
            None => dropped.push(add),
        }
    }
    let mut warnings = old.warnings;
    for warning in new.warnings {
        if !warnings.contains(&warning) {
            warnings.push(warning);
        }
    }
    HistoryReport {
        fidelity: old.fidelity,
        items_total: old.items_total + new.items_total,
        by_kind,
        dropped,
        warnings,
    }
}

/// A report as the segment stores it: the report, and beside it what the segment ran on where it was made.
fn stored(report: &HistoryReport, imported: Json) -> Json {
    let mut value = serde_json::to_value(report).unwrap_or(Json::Null);
    if let Some(map) = value.as_object_mut() {
        map.insert("imported".to_owned(), imported);
    }
    value
}

/// The members of what the source's segment ran on that are kept, by name.
fn kept_of(segment: Option<&Json>, thing: &str, members: &[&str]) -> Json {
    let source = segment.and_then(|segment| segment.get(thing));
    let mut kept = serde_json::Map::new();
    for member in members {
        if let Some(text) = source
            .and_then(|source| source.get(*member))
            .and_then(Json::as_str)
        {
            kept.insert((*member).to_owned(), Json::String(text.to_owned()));
        }
    }
    Json::Object(kept)
}

/// The word `asked` where it is one of `words`, else `otherwise`.
fn word_among(
    asked: Option<&str>,
    words: &[&'static str],
    otherwise: &'static str,
) -> &'static str {
    asked
        .and_then(|asked| words.iter().copied().find(|word| *word == asked))
        .unwrap_or(otherwise)
}

/// The moments a segment of the history began and ended: those of the source's first and last segments, else of its first and last
/// items (`items`), else `now`.
fn span_of(
    segments: &[Json],
    items: Option<(DateTime<Utc>, DateTime<Utc>)>,
    now: DateTime<Utc>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    let began = segments
        .first()
        .and_then(|first| time_of(first.get("started_at")))
        .or_else(|| items.map(|(began, _)| began))
        .unwrap_or(now);
    let ended = segments
        .last()
        .and_then(|last| time_of(last.get("ended_at")))
        .or_else(|| items.map(|(_, ended)| ended))
        .unwrap_or(now);
    (began, ended.max(began))
}

// ---------------------------------------------------------------- writing

/// Cuts a run of items into the pages a statement writes each: no more than a page of items and a page of bytes, and an item larger
/// than a page of bytes by itself.
fn pages(sizes: &[usize]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut bytes = 0_usize;
    for (at, size) in sizes.iter().enumerate() {
        let full = at - start >= ITEMS_PER_STATEMENT
            || (at > start && bytes.saturating_add(*size) > STATEMENT_BYTES_MAX);
        if full {
            out.push(start..at);
            start = at;
            bytes = 0;
        }
        bytes = bytes.saturating_add(*size);
    }
    if start < sizes.len() {
        out.push(start..sizes.len());
    }
    out
}

/// A text cut to `most` characters and trimmed.
fn cut(text: &str, most: usize) -> String {
    text.chars()
        .take(most)
        .collect::<String>()
        .trim()
        .to_owned()
}

// ---------------------------------------------------------------- reading and writing the record, a page at a time

/// How many items of the record are read, adapted and written together: what one statement writes.
const READ_PAGE: usize = ITEMS_PER_STATEMENT;
/// How many bytes of the record a page may take, once it holds that much, however few items that is.
const READ_PAGE_BYTES: u64 = 4 * 1024 * 1024;
/// How many things a run remembers to spare asking again (the files known to be here, and those known not to be): past it the memory is
/// let go and the store is asked again, which costs a statement and nothing else.
const REMEMBERED_MAX: usize = 100_000;

/// What the history of a thread comes to as its record is read: the page of items that is being written, and what is counted of all of
/// them. The memory of it is a page and the files it remembers, whatever the record's size.
struct History<'a, 'b> {
    batch: &'a Batch<'b>,
    object: &'a Object,
    thread: Id,
    /// Whether the rows that say what the import added are written: when it adds to a thread that was there.
    origins: bool,
    /// The segment of the imported history.
    segment: Id,
    /// The first of the two numbers of the thread's sequence taken for its state and its segment: the segment is stamped with the second.
    first: Seq,
    /// The segments the record lists, which come before its items.
    segments: Vec<Json>,
    /// How many turns the record holds, which are not made again.
    turns_in_record: i64,
    /// What the items are found by in a search, loaded once.
    texts: ItemTexts,
    /// The files the batch carried.
    carried: HashSet<Hash>,
    /// The files known to be here.
    here: HashSet<Hash>,
    /// The files known not to be here.
    absent: HashSet<Hash>,
    lost: Lost,
    tally: Tally,
    /// How many different files the items name that are here.
    files: i64,
    /// Whether the segment is there, and whether this run made it.
    made_segment: Option<bool>,
    /// How many items were written.
    items: i64,
    /// The turns the items written were made in.
    turns: HashSet<Id>,
    /// The moment of the latest item written.
    latest: Option<DateTime<Utc>>,
    /// The number of the thread's sequence of the last item written.
    last_seq: Seq,
}

/// What bringing the history of a thread came to.
struct Brought {
    tally: Tally,
    files: i64,
    items: i64,
    turns: i64,
    last_seq: Seq,
}

impl History<'_, '_> {
    /// A member of the record that is not read a page at a time.
    fn member(&mut self, name: &str, value: Json) {
        if name == "segments"
            && let Json::Array(list) = value
        {
            self.segments = list;
        }
    }

    /// What the segment ran on where it was made, with the import and the thread it came from.
    fn imported(&self) -> Json {
        let last = self.segments.last();
        json!({
            "import": self.batch.import,
            "package": self.batch.plan.opened.package,
            "thread": self.object.object.id,
            "turns": self.turns_in_record,
            "segments": self.segments.len(),
            "agent": kept_of(last, "agent", &["name", "harness_kind"]),
            "provider": kept_of(last, "provider", &["name", "kind", "harness_kind"]),
            "environment": kept_of(last, "environment", &["name"]),
        })
    }

    /// A page of the items of the record: read, adapted and written.
    async fn items_page(&mut self, tx: &mut Tx, page: Vec<Json>) -> Result<(), MakeError> {
        let thread = self.thread;
        let (candidates, unreadable) = candidates_in(&page);
        drop(page);
        for _ in 0..unreadable {
            self.tally.left_out(UNKNOWN_KIND, DROPPED_UNREADABLE);
        }
        if candidates.is_empty() {
            return Ok(());
        }
        let ids: Vec<Id> = candidates
            .iter()
            .map(|each| identity::inside(thread, ITEM_ROW, each.source))
            .collect();
        // What an earlier import brought is there already, and is not counted again.
        let present = rows::present_items(tx, thread, &ids).await?;
        self.know_files(tx, &candidates, &ids, &present).await?;
        let mut ready: Vec<Ready> = Vec::with_capacity(candidates.len());
        {
            let with = Adapt {
                thread,
                here: &self.here,
                credits: &self.batch.plan.credits,
                now: self.batch.now,
            };
            for (each, id) in candidates.into_iter().zip(ids) {
                if present.contains(&id) {
                    continue;
                }
                let kind = each.kind.clone();
                if let Some(why) = why_left_out(&each) {
                    self.tally.left_out(&kind, why);
                    continue;
                }
                match adapt(each, &with, &mut self.lost) {
                    Ok(item) => {
                        self.tally.kept(&kind, item.changed);
                        ready.push(item);
                    }
                    Err(why) => self.tally.left_out(&kind, why),
                }
            }
        }
        self.write(tx, ready).await?;
        self.batch
            .say(|| format!("a conversation: {} items brought", self.items));
        Ok(())
    }

    /// Asks the store which of the files the items to be written name are here: those the batch carried and the store holds, and those the
    /// thread holds already.
    async fn know_files(
        &mut self,
        tx: &mut Tx,
        candidates: &[Candidate],
        ids: &[Id],
        present: &HashSet<Id>,
    ) -> Result<(), MakeError> {
        if self.here.len() + self.absent.len() > REMEMBERED_MAX {
            self.here.clear();
            self.absent.clear();
        }
        let mut named = Vec::new();
        for (each, id) in candidates.iter().zip(ids) {
            if !present.contains(id) {
                blobs_in(&each.payload, &mut named);
            }
        }
        named.sort_unstable();
        named.dedup();
        named.retain(|hash| !self.here.contains(hash) && !self.absent.contains(hash));
        if named.is_empty() {
            return Ok(());
        }
        // A file counts as brought where the package carried it, or where the thread holds it already: a package may not make the
        // store's other files readable through a thread by naming their hashes.
        let carried: Vec<Hash> = named
            .iter()
            .copied()
            .filter(|hash| self.carried.contains(hash))
            .collect();
        let mut found = rows::present_blobs(tx, &carried).await?;
        let rest: Vec<Hash> = named
            .iter()
            .copied()
            .filter(|hash| !found.contains(hash))
            .collect();
        found.extend(rows::held_blobs(tx, self.thread, &rest).await?);
        for hash in named {
            if found.contains(&hash) {
                self.here.insert(hash);
                self.files += 1;
            } else {
                self.absent.insert(hash);
            }
        }
        Ok(())
    }

    /// Writes the segment on the first page that has something to write.
    async fn segment(&mut self, tx: &mut Tx, ready: &[Ready]) -> Result<(), MakeError> {
        if self.made_segment.is_some() {
            return Ok(());
        }
        let span = ready
            .iter()
            .map(|item| item.created_at)
            .min()
            .zip(ready.iter().map(|item| item.updated_at).max());
        let (began, ended) = span_of(&self.segments, span, self.batch.now);
        let last = self.segments.last();
        let arrangement = word_among(
            last.and_then(|segment| segment.get("arrangement"))
                .and_then(Json::as_str),
            &ARRANGEMENTS,
            NEUTRAL_ARRANGEMENT,
        );
        let credential_path = word_among(
            last.and_then(|segment| segment.get("credential_path"))
                .and_then(Json::as_str),
            &CREDENTIAL_PATHS,
            NEUTRAL_CREDENTIAL_PATH,
        );
        // It says what it holds so far, which is nothing; the account of all of it is written when the last page is.
        let report = stored(&Tally::default().into_report(), self.imported());
        let made = if rows::placed_segment(tx, self.thread, self.segment)
            .await?
            .is_some()
        {
            false
        } else {
            rows::put_segment(
                tx,
                &rows::NewSegment {
                    id: self.segment,
                    thread: self.thread,
                    started_by: self.batch.principal.user(),
                    started_at: began,
                    ended_at: ended,
                    arrangement,
                    credential_path,
                    report: &report,
                    seq: self.first + 1,
                },
            )
            .await?;
            true
        };
        self.made_segment = Some(made);
        Ok(())
    }

    /// Writes the items of a page that are not here, under the numbers of the thread's sequence that follow, and the rows that say what
    /// they are.
    async fn write(&mut self, tx: &mut Tx, mut ready: Vec<Ready>) -> Result<(), MakeError> {
        if ready.is_empty() {
            return Ok(());
        }
        self.segment(tx, &ready).await?;
        let thread = self.thread;
        // What it is found by in a search: the words of what it says, where it says any.
        for item in &mut ready {
            if crate::projections::plain_item(&item.kind, &item.payload).is_some() {
                item.search_text = self
                    .texts
                    .text(tx, thread, &item.kind, &item.payload)
                    .await?;
            }
        }
        // An item stands under the one it was made under, where that is here already or was written before it in this page.
        let wanted: Vec<Id> = ready
            .iter()
            .filter_map(|item| item.parent)
            .map(|parent| identity::inside(thread, ITEM_ROW, parent))
            .collect();
        let mut known = rows::present_items(tx, thread, &wanted).await?;
        let count = i64::try_from(ready.len()).unwrap_or(i64::MAX);
        let Some(block) = rows::take_seqs(tx, thread, count).await? else {
            return refuse("the thread is gone");
        };
        let sizes: Vec<usize> = ready.iter().map(|item| item.size).collect();
        let mut inserted: HashSet<Id> = HashSet::new();
        let mut at_seq = block;
        for page in pages(&sizes) {
            let slice = &ready[page];
            let refs: Vec<String> = slice
                .iter()
                .map(|item| format!("import:{}", item.source))
                .collect();
            let mut rows_of_page = Vec::with_capacity(slice.len());
            for (item, native_ref) in slice.iter().zip(&refs) {
                let parent = item
                    .parent
                    .map(|parent| identity::inside(thread, ITEM_ROW, parent))
                    .filter(|parent| known.contains(parent));
                known.insert(item.id);
                rows_of_page.push(rows::ImportedItem {
                    id: item.id,
                    parent,
                    stream: &item.stream,
                    kind: &item.kind,
                    status: item.status,
                    author: item.author,
                    payload: &item.payload,
                    native_ref,
                    search_text: item.search_text.as_deref(),
                    seq: at_seq,
                    created_at: item.created_at,
                    updated_at: item.updated_at,
                });
                at_seq += 1;
            }
            inserted.extend(rows::put_items(tx, thread, self.segment, &rows_of_page).await?);
        }
        self.last_seq = at_seq - 1;
        let kept: Vec<&Ready> = ready
            .iter()
            .filter(|item| inserted.contains(&item.id))
            .collect();
        if self.origins {
            let made: Vec<OriginRow> = kept
                .iter()
                .map(|item| origin_of(self.batch, thread, item))
                .collect();
            rows::put_item_origins(tx, &made).await?;
        }
        for item in &kept {
            self.latest = Some(
                self.latest
                    .map_or(item.updated_at, |before| before.max(item.updated_at)),
            );
            if let Some(turn) = item.turn {
                self.turns.insert(turn);
            }
        }
        self.items += i64::try_from(kept.len()).unwrap_or(i64::MAX);
        Ok(())
    }

    /// Writes what the segment says of all it holds, together with what an earlier import said of it, and moves its end to the last of
    /// its items.
    async fn finish(&mut self, tx: &mut Tx) -> Result<(), MakeError> {
        let Some(made) = self.made_segment else {
            return Ok(());
        };
        let report = self.tally.clone().into_report();
        let imported = self.imported();
        let placed = rows::placed_segment(tx, self.thread, self.segment)
            .await?
            .and_then(|placed| placed.report);
        let value = match placed {
            Some(old) => {
                // The account of what it ran on is the first import's, unless this run wrote the segment.
                let kept = if made {
                    imported
                } else {
                    old.get("imported").cloned().unwrap_or(imported)
                };
                let together = serde_json::from_value::<HistoryReport>(old)
                    .ok()
                    .map_or_else(|| report.clone(), |old| merged(old, report.clone()));
                stored(&together, kept)
            }
            None => stored(&report, imported),
        };
        rows::set_segment_report(tx, self.segment, &value, self.first + 1).await?;
        if let Some(latest) = self.latest {
            rows::extend_segment(tx, self.segment, latest).await?;
        }
        Ok(())
    }
}

/// Reads the record of a thread from its copy a page at a time and writes the history it holds.
async fn bring_history(
    tx: &mut Tx,
    batch: &Batch<'_>,
    object: &Object,
    record: &Record,
    first: Seq,
    origins: bool,
) -> Result<Brought, MakeError> {
    let thread = object.target;
    let mut history = History {
        batch,
        object,
        thread,
        origins,
        segment: identity::inside(thread, SEGMENT_ROW, object.object.id),
        first,
        segments: Vec::new(),
        turns_in_record: 0,
        texts: ItemTexts::of(tx, thread).await?,
        carried: batch.staged.values().map(|staged| staged.blob).collect(),
        here: HashSet::new(),
        absent: HashSet::new(),
        lost: Lost::default(),
        tally: Tally::default(),
        files: 0,
        made_segment: None,
        items: 0,
        turns: HashSet::new(),
        latest: None,
        last_seq: first + 1,
    };
    let mut pieces = records::read_object(
        record.file()?,
        &["turns", "items"],
        Cut {
            elements: READ_PAGE,
            bytes: READ_PAGE_BYTES,
        },
    );
    while let Some(piece) = pieces.recv().await {
        let piece = piece
            .map_err(|error| MakeError::Refused(format!("its record cannot be read: {error}")))?;
        match piece {
            Piece::Member(name, value) => history.member(&name, value),
            Piece::Page(name, page) if name == "turns" => {
                history.turns_in_record += i64::try_from(page.len()).unwrap_or(i64::MAX);
            }
            Piece::Page(_, page) => history.items_page(tx, page).await?,
        }
    }
    history.finish(tx).await?;
    if let Some(latest) = history.latest {
        threads::touch(tx, thread, latest.min(batch.now)).await?;
    }
    history.tally.files_replaced = history.lost.replaced;
    history.tally.files_dangling = history.lost.dangling;
    Ok(Brought {
        files: history.files,
        items: history.items,
        turns: i64::try_from(history.turns.len()).unwrap_or(i64::MAX),
        last_seq: history.last_seq,
        tally: history.tally,
    })
}

/// The row of `import_origins` for an item added to a thread that was there.
fn origin_of(batch: &Batch<'_>, thread: Id, item: &Ready) -> OriginRow {
    let credits = &batch.plan.credits;
    OriginRow {
        scope_id: batch.plan.destination.id,
        source_id: item.source,
        kind: origin_kind::ITEM.to_owned(),
        object_id: item.id,
        parent_id: Some(thread),
        created_by_import: Some(batch.import),
        last_import: batch.import,
        source_name: None,
        source_path: None,
        source_sha256: None,
        source_updated_at: Some(item.updated_at),
        source_creator: item
            .by
            .map(|who| json!({ "name": credits.name(who), "username": credits.username(who) })),
        imported_by: Some(batch.principal.user()),
        imported_at: batch.now,
    }
}

/// Makes the node of a thread under its topic: its page and owner, its chat, its description and its times, its place in the feeds
/// of the tree, the line of the audit, and where it came from.
async fn make_node(tx: &mut Tx, batch: &Batch<'_>, object: &Object) -> Result<(), MakeError> {
    let importer = batch.principal.user();
    let thread = object.target;
    let mut name = cut(&object.object.name, NAME_MAX);
    if name.is_empty() {
        name = UNNAMED_THREAD.to_owned();
    }
    if object.action == ImportAction::KeepBoth {
        name = free_name(
            tx,
            PackageKind::Thread,
            object.lies_in,
            None,
            &name,
            NAME_MAX,
        )
        .await?;
    }
    let owner = orch_authz::ownership::owner_of_new(tx, importer, Some(object.lies_in)).await?;
    let at = object
        .object
        .created_at
        .map_or(batch.now, |created| created.get());
    if !threads::insert_thread(tx, thread, object.lies_in, &name, owner, at).await? {
        return refuse("the topic it lies in is not there any more");
    }
    orch_authz::pages::start(tx, StoredKind::Node, thread, importer, Time::new(batch.now)).await?;
    threads::make_node_chat(tx, thread, importer, at).await?;
    // What it says of itself, and the time it last changed there, are what it has here.
    let description = cut(&object.object.description, DESCRIPTION_MAX);
    rows::describe_node(
        tx,
        thread,
        (!description.is_empty()).then_some(description.as_str()),
        object.object.updated_at.map(Time::get),
    )
    .await?;
    hierarchy::stamp(tx, thread).await?;
    hierarchy::stamp(tx, object.lies_in).await?;
    let mut audit = AuditEntry::new(
        Actor::User(importer),
        orch_db::audit::action!(AuditAction::ThreadCreated).as_str(),
        "thread",
        Some(thread),
        "A thread was made by an import.",
    );
    audit.history = AuditAction::ThreadCreated.is_history_line();
    audit.node_id = Some(thread);
    audit.org_id = Some(batch.plan.destination.org);
    audit.details = json!({ "name": name, "parent": object.lies_in, "import": batch.import });
    append(tx, &audit, &orch_db::audit::Origin::default(), batch.now).await?;
    note_origin(tx, batch, object, thread, Some(object.lies_in), true).await?;
    Ok(())
}

/// What a thing that was left out or lost is worth to the one who reads the report of the import.
fn warnings_of(tally: &Tally) -> Vec<ImportWarning> {
    let mut warnings = Vec::new();
    if tally.how_many_left_out() > 0 {
        warnings.push(ImportWarning {
            code: "history_dropped".to_owned(),
            message: format!(
                "{} items of the conversation do not come: lines about the thread's own changes, requests for approval and items that cannot be read",
                tally.how_many_left_out()
            ),
            params: Some(json!({ "count": tally.how_many_left_out() })),
        });
    }
    if tally.files_lost() > 0 {
        warnings.push(ImportWarning {
            code: "missing_file".to_owned(),
            message: format!(
                "{} files the conversation names were not part of the package",
                tally.files_lost()
            ),
            params: Some(json!({ "count": tally.files_lost() })),
        });
    }
    warnings
}

/// Makes a thread, or takes over the one the plan says is there, and brings its history.
pub(super) async fn make_thread(
    tx: &mut Tx,
    batch: &Batch<'_>,
    object: &Object,
) -> Result<Made, MakeError> {
    let thread = object.target;
    let merging = object.action == ImportAction::Replace;
    // What it brings is copied and checked against its hash before anything is written: a record that is damaged makes nothing.
    let record = if object.bare {
        None
    } else {
        Some(Record::open(batch, object, "thread").await?)
    };
    let held = rows::held(tx, thread).await?;
    match held {
        Some(held) if held.side => return refuse("an aside takes no history of its own"),
        Some(held) if held.deleted => return refuse("the thread that was there is gone"),
        None if merging => return refuse("the thread that was there is gone"),
        _ => {}
    }
    let created = held.is_none();
    if created {
        make_node(tx, batch, object).await?;
    } else {
        // A thread that is there: the one an earlier import made, which a retry finds, or one that takes this history in.
        note_origin(tx, batch, object, thread, Some(object.lies_in), !merging).await?;
    }
    // Two numbers of the thread's sequence, for its state and its segment; each page of items takes the numbers it needs.
    let Some(first) = rows::take_seqs(tx, thread, SEQS_BESIDES_ITEMS).await? else {
        return refuse("the thread is gone");
    };
    if created {
        threads::set_state(
            tx,
            thread,
            &[thread_state::IDLE],
            &StateChange::blocked(UNBOUND, Some(UNBOUND_DETAIL)),
            first,
        )
        .await?;
    }
    let mut made = Made::done(object, thread);
    made.author = object
        .object
        .creator
        .and_then(|who| batch.plan.credits.of(Some(who)).original);
    if let Some(record) = &record {
        let brought = bring_history(tx, batch, object, record, first, merging).await?;
        made.warnings.extend(warnings_of(&brought.tally));
        made.carried.files = brought.files;
        made.carried.turns = brought.turns;
        if brought.items == 0 && merging {
            made.outcome = ImportOutcome::Unchanged;
        }
        if brought.items > 0 && !created {
            // Readers of a thread that was there are told that it has more.
            orch_db::sequences::notify(tx, thread, brought.last_seq).await?;
        }
    }
    Ok(made)
}

/// Takes back a batch of the items that the import `import` added to the thread `thread`, as the person `principal` may now: how many rows
/// of the import it looked at. None are left when `Taken::Nothing` is the answer; `Taken::Refused` says the person may not write in the
/// thread now, and what the import added to it stays.
pub(super) async fn take_back(
    tx: &mut Tx,
    import: Id,
    thread: Id,
    principal: &orch_authz::Principal,
    limit: i64,
) -> Result<Taken, MakeError> {
    // What lies in a place that was deleted goes and comes back with it.
    if !place_is_live(tx, thread).await? {
        return Ok(Taken::Nothing);
    }
    let allowed = matches!(
        orch_authz::nodes::check(
            tx,
            principal,
            thread,
            orch_access_contract::permissions::Permission::ThreadInteract,
            orch_authz::Act::Use,
            orch_authz::Policy::default(),
        )
        .await,
        Ok(orch_access_contract::decisions::Decision::Allowed { .. })
    );
    if !allowed {
        return Ok(Taken::Refused);
    }
    let looked = rows::take_back_items(tx, import, thread, limit).await?;
    Ok(if looked == 0 {
        Taken::Nothing
    } else {
        Taken::Some(looked)
    })
}

/// Takes back the segments of imported history that the import wrote for threads that were there and that hold nothing now; asked
/// once, after the last batch of items.
pub(super) async fn take_back_finish(tx: &mut Tx, import: Id) -> Result<u64, MakeError> {
    Ok(rows::take_back_segments(tx, import).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    fn candidate_json(kind: &str, status: &str, seq: i64, payload: Json) -> Json {
        json!({
            "id": new_id(), "kind": kind, "status": status, "seq_created": seq, "payload": payload,
            "stream": "main", "created_at": "2026-03-01T10:00:00.000Z", "updated_at": "2026-03-01T10:00:01.000Z",
        })
    }

    fn said(text: &str) -> Json {
        json!({ "parts": [{ "type": "text", "text": text }], "input_id": null, "delivery": "queue", "source": "user" })
    }

    fn context<'a>(here: &'a HashSet<Hash>, credits: &'a Credits) -> Adapt<'a> {
        Adapt {
            thread: new_id(),
            here,
            credits,
            now: "2026-10-01T00:00:00Z".parse().unwrap(),
        }
    }

    fn nobody() -> Credits {
        Credits::new(
            new_id(),
            orch_api_contract::imports::ImportAuthors::Labels,
            Vec::new(),
        )
    }

    #[test]
    fn items_are_read_in_the_order_of_the_record_and_what_is_no_item_is_counted() {
        let (late, early) = (
            candidate_json("agent_message", "completed", 9, json!({})),
            candidate_json("agent_message", "completed", 2, json!({})),
        );
        let page = [
            late.clone(),
            json!({ "kind": "marker" }),
            early.clone(),
            json!("x"),
        ];
        let (found, unreadable) = candidates_in(&page);
        assert_eq!(unreadable, 2);
        assert_eq!(found.len(), 2);
        assert_eq!(
            found[0].source.to_string(),
            late["id"].as_str().unwrap(),
            "the export writes the conversation in the order it was made, and it is read in that order"
        );
        assert_eq!(found[1].source.to_string(), early["id"].as_str().unwrap());
        assert_eq!(candidates_in(&[]).0.len(), 0);
    }

    #[test]
    fn what_was_under_way_when_the_source_was_written_never_finishes_and_what_waited_is_not_waiting()
     {
        assert_eq!(status_here("in_progress"), Some("cancelled"));
        assert_eq!(status_here("pending"), Some("expired"));
        for word in [
            "completed",
            "failed",
            "cancelled",
            "approved",
            "answered",
            "denied",
            "expired",
        ] {
            assert_eq!(status_here(word), Some(word));
        }
        assert_eq!(status_here("running"), None);
    }

    #[test]
    fn lines_about_the_sources_own_acts_and_requests_for_approval_are_left_out_and_the_rest_stays()
    {
        let read = |kind: &str, status: &str| {
            candidate(&candidate_json(kind, status, 1, json!({}))).unwrap()
        };
        assert_eq!(
            why_left_out(&read("marker", "completed")),
            Some(DROPPED_MARKER)
        );
        assert_eq!(
            why_left_out(&read("approval", "pending")),
            Some(DROPPED_APPROVAL)
        );
        assert_eq!(
            why_left_out(&read("weather", "completed")),
            Some(DROPPED_UNREADABLE)
        );
        assert_eq!(
            why_left_out(&read("agent_message", "running")),
            Some(DROPPED_UNREADABLE)
        );
        for kind in [
            "user_message",
            "agent_message",
            "reasoning",
            "plan",
            "tool_call",
            "announcement",
            "error",
        ] {
            assert_eq!(why_left_out(&read(kind, "completed")), None, "{kind}");
        }
    }

    #[test]
    fn every_file_a_payload_names_is_found_wherever_it_is_named() {
        let (one, two, three) = (Hash::of(b"one"), Hash::of(b"two"), Hash::of(b"three"));
        let payload = json!({
            "parts": [
                { "type": "image", "blob": one, "name": "a.png" },
                { "type": "forwarded", "images": [{ "blob": two, "name": "b.png" }] },
            ],
            "output": { "inline": "x", "blob": three },
            "blob": "not a hash",
            "other": { "blob": null },
        });
        let mut found = Vec::new();
        blobs_in(&payload, &mut found);
        found.sort_unstable();
        let mut expected = vec![one, two, three];
        expected.sort_unstable();
        assert_eq!(found, expected);
    }

    #[test]
    fn a_file_that_did_not_come_is_words_where_it_was_an_image_and_nothing_where_it_was_a_forwarded_one()
     {
        let (here, gone, also_gone) = (Hash::of(b"here"), Hash::of(b"gone"), Hash::of(b"also"));
        let held: HashSet<Hash> = [here].into_iter().collect();
        let mut payload = json!({
            "parts": [
                { "type": "image", "blob": here, "name": "kept.png" },
                { "type": "image", "blob": gone, "name": "lost.png" },
                { "type": "pasted_text", "blob": gone, "name": "paste.txt", "inline_text": "what was pasted" },
                { "type": "pasted_text", "blob": gone, "name": "long.txt" },
                { "type": "file", "blob": gone, "name": "data.csv", "path": "data.csv", "size": 3 },
                { "type": "forwarded", "images": [{ "blob": gone, "name": "f.png" }, { "blob": here, "name": "g.png" }],
                  "attachments": [{ "blob": also_gone, "name": "f.zip" }] },
            ],
        });
        let mut lost = Lost::default();
        degrade(&mut payload, &held, &mut lost);
        let parts = payload["parts"].as_array().unwrap();
        assert_eq!(parts[0]["blob"], json!(here));
        assert_eq!(
            parts[1],
            json!({ "type": "text", "text": "The file \"lost.png\" was not brought." })
        );
        assert_eq!(
            parts[2],
            json!({ "type": "text", "text": "what was pasted" })
        );
        assert_eq!(parts[3]["text"], "The file \"long.txt\" was not brought.");
        assert_eq!(parts[4]["blob"], Json::Null);
        assert_eq!(parts[4]["name"], "data.csv");
        assert_eq!(parts[5]["images"].as_array().unwrap().len(), 1);
        assert_eq!(parts[5]["images"][0]["blob"], json!(here));
        assert_eq!(parts[5]["attachments"].as_array().unwrap().len(), 0);
        assert_eq!(
            lost,
            Lost {
                replaced: 6,
                dangling: 0
            }
        );
    }

    #[test]
    fn a_long_output_whose_file_did_not_come_is_cut_and_a_file_that_cannot_be_without_stays_and_is_counted()
     {
        let (gone, held) = (Hash::of(b"gone"), HashSet::new());
        let mut payload = json!({
            "tool": { "kind": "command", "output": { "inline": "tail", "total_len": 9, "truncated": true, "blob": gone } },
            "image": { "kind": "image", "blob": gone, "media_type": "image/png" },
        });
        let mut lost = Lost::default();
        degrade(&mut payload, &held, &mut lost);
        assert_eq!(payload["tool"]["output"]["blob"], Json::Null);
        assert_eq!(payload["image"]["blob"], json!(gone));
        assert_eq!(
            lost,
            Lost {
                replaced: 1,
                dangling: 1
            }
        );
    }

    #[test]
    fn a_message_by_somebody_the_importer_may_not_write_as_says_who_wrote_it_and_is_written_as_the_importer()
     {
        let importer = new_id();
        let credits = Credits::new(
            importer,
            orch_api_contract::imports::ImportAuthors::Labels,
            Vec::new(),
        );
        let here = HashSet::new();
        let with = Adapt {
            thread: new_id(),
            here: &here,
            credits: &credits,
            now: "2026-10-01T00:00:00Z".parse().unwrap(),
        };
        // A person the package does not name is an imported person, and the importer writes what they said.
        let mut entry = candidate_json("user_message", "completed", 1, said("hello"));
        entry["author"] = json!({ "id": new_id() });
        let mut lost = Lost::default();
        let item = adapt(candidate(&entry).unwrap(), &with, &mut lost).unwrap();
        assert_eq!(item.author, Some(importer));
        assert!(item.changed);
        let parts = item.payload["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["text"], "Originally by Imported person");
        assert_eq!(parts[1]["text"], "hello");
        assert_eq!(item.payload["input_id"], Json::Null);
    }

    #[test]
    fn what_the_importer_said_themself_has_no_label_and_a_message_without_an_author_has_no_account()
    {
        let importer = new_id();
        let credits = Credits::new(
            importer,
            orch_api_contract::imports::ImportAuthors::Labels,
            Vec::new(),
        );
        let here = HashSet::new();
        let with = Adapt {
            thread: new_id(),
            here: &here,
            credits: &credits,
            now: "2026-10-01T00:00:00Z".parse().unwrap(),
        };
        let entry = candidate_json("user_message", "completed", 1, said("hello"));
        let mut lost = Lost::default();
        let item = adapt(candidate(&entry).unwrap(), &with, &mut lost).unwrap();
        assert_eq!(item.author, None);
        assert!(!item.changed);
        assert_eq!(item.payload["parts"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn an_item_is_written_with_the_identifier_its_thread_and_source_give_and_the_times_of_the_source()
     {
        let (here, credits) = (HashSet::new(), nobody());
        let with = context(&here, &credits);
        let entry = candidate_json(
            "agent_message",
            "in_progress",
            4,
            json!({ "text": "hi", "text_len": 2, "phase": null }),
        );
        let mut lost = Lost::default();
        let one = adapt(candidate(&entry).unwrap(), &with, &mut lost).unwrap();
        let again = adapt(candidate(&entry).unwrap(), &with, &mut lost).unwrap();
        assert_eq!(one.id, again.id);
        assert_eq!(one.id, identity::inside(with.thread, ITEM_ROW, one.source));
        assert_eq!(one.status, "cancelled");
        assert!(one.changed);
        assert_eq!(one.created_at.to_rfc3339(), "2026-03-01T10:00:00+00:00");
        assert!(one.updated_at > one.created_at);
        assert!(one.author.is_none());
    }

    #[test]
    fn a_plan_loses_the_approval_it_was_decided_by_and_an_item_that_does_not_read_is_left_out() {
        let (here, credits) = (HashSet::new(), nobody());
        let with = context(&here, &credits);
        let plan = candidate_json(
            "plan",
            "completed",
            1,
            json!({
                "explanation": null, "steps": [{ "text": "a", "status": "pending" }],
                "approval_id": new_id(), "decision": { "state": "approved" },
            }),
        );
        let mut lost = Lost::default();
        let item = adapt(candidate(&plan).unwrap(), &with, &mut lost).unwrap();
        assert!(item.payload.get("approval_id").is_none());
        assert!(item.payload.get("decision").is_none());
        assert!(!item.changed);
        let broken = candidate_json(
            "agent_message",
            "completed",
            1,
            json!({ "words": "no text" }),
        );
        assert_eq!(
            adapt(candidate(&broken).unwrap(), &with, &mut lost).unwrap_err(),
            DROPPED_UNREADABLE
        );
    }

    #[test]
    fn pages_hold_a_page_of_items_or_a_page_of_bytes_and_an_item_larger_than_that_stands_alone() {
        assert!(pages(&[]).is_empty());
        assert_eq!(pages(&[1, 1, 1]), vec![0..3]);
        let many = vec![1; ITEMS_PER_STATEMENT * 2 + 1];
        assert_eq!(
            pages(&many),
            vec![
                0..ITEMS_PER_STATEMENT,
                ITEMS_PER_STATEMENT..ITEMS_PER_STATEMENT * 2,
                ITEMS_PER_STATEMENT * 2..many.len()
            ]
        );
        let huge = STATEMENT_BYTES_MAX + 1;
        assert_eq!(pages(&[10, huge, 10, 10]), vec![0..1, 1..2, 2..4]);
        let half = STATEMENT_BYTES_MAX / 2;
        assert_eq!(pages(&[half, half, half]), vec![0..2, 2..3]);
    }

    #[test]
    fn a_report_counts_what_was_kept_converted_and_left_out_by_kind_and_why() {
        let mut tally = Tally::default();
        tally.kept("agent_message", false);
        tally.kept("agent_message", true);
        tally.kept("tool_call", false);
        tally.left_out("marker", DROPPED_MARKER);
        tally.left_out("marker", DROPPED_MARKER);
        tally.left_out("approval", DROPPED_APPROVAL);
        tally.files_replaced = 1;
        tally.files_dangling = 2;
        assert_eq!(tally.how_many_left_out(), 3);
        let report = tally.into_report();
        assert_eq!(report.fidelity, Fidelity::Structured);
        assert_eq!(report.items_total, 6);
        assert_eq!(
            report.by_kind["agent_message"],
            ReportKind {
                kept: 1,
                converted: 1,
                dropped: 0
            }
        );
        assert_eq!(report.by_kind["marker"].dropped, 2);
        assert_eq!(report.dropped.len(), 2);
        assert!(
            report
                .dropped
                .iter()
                .any(|row| row.kind == "marker" && row.count == 2 && row.reason == DROPPED_MARKER)
        );
        assert_eq!(report.warnings.len(), 2);
        assert_eq!(report.warnings[0], HISTORY_ONLY);
        assert!(report.warnings[1].starts_with("3 references"));
    }

    #[test]
    fn two_reports_of_one_segment_add_up_and_say_a_warning_once() {
        let mut one = Tally::default();
        one.kept("agent_message", false);
        one.left_out("marker", DROPPED_MARKER);
        let mut two = Tally::default();
        two.kept("agent_message", false);
        two.kept("user_message", true);
        two.left_out("marker", DROPPED_MARKER);
        two.left_out("approval", DROPPED_APPROVAL);
        let together = merged(one.into_report(), two.into_report());
        assert_eq!(together.items_total, 6);
        assert_eq!(together.by_kind["agent_message"].kept, 2);
        assert_eq!(together.by_kind["user_message"].converted, 1);
        assert_eq!(together.by_kind["marker"].dropped, 2);
        let markers: Vec<_> = together
            .dropped
            .iter()
            .filter(|row| row.kind == "marker")
            .collect();
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].count, 2);
        assert_eq!(together.dropped.len(), 2);
        assert_eq!(together.warnings, vec![HISTORY_ONLY.to_owned()]);
    }

    #[test]
    fn what_a_report_is_stored_with_reads_back_as_a_report_and_names_what_the_segment_ran_on() {
        let imported = json!({ "agent": { "name": "Reviewer" } });
        let mut tally = Tally::default();
        tally.kept("user_message", false);
        let report = tally.into_report();
        let value = stored(&report, imported.clone());
        assert_eq!(value["imported"], imported);
        assert_eq!(value["fidelity"], "structured");
        let read: HistoryReport = serde_json::from_value(value).unwrap();
        assert_eq!(read, report);
    }

    #[test]
    fn what_the_last_segment_ran_on_is_kept_by_name_and_nothing_else_of_it() {
        let segment = json!({
            "agent": { "id": new_id(), "name": "Reviewer", "address": "a.b", "harness_kind": "claude", "machine": null },
            "provider": { "id": new_id(), "name": "Plan", "kind": "subscription" },
        });
        let agent = kept_of(Some(&segment), "agent", &["name", "harness_kind"]);
        assert_eq!(
            agent,
            json!({ "name": "Reviewer", "harness_kind": "claude" })
        );
        assert_eq!(
            kept_of(
                Some(&segment),
                "provider",
                &["name", "kind", "harness_kind"]
            ),
            json!({ "name": "Plan", "kind": "subscription" })
        );
        assert_eq!(kept_of(Some(&segment), "environment", &["name"]), json!({}));
        assert_eq!(kept_of(None, "agent", &["name"]), json!({}));
    }

    #[test]
    fn a_word_the_columns_do_not_hold_is_a_neutral_one() {
        assert_eq!(
            word_among(Some("separated"), &ARRANGEMENTS, NEUTRAL_ARRANGEMENT),
            "separated"
        );
        assert_eq!(
            word_among(Some("elsewhere"), &ARRANGEMENTS, NEUTRAL_ARRANGEMENT),
            NEUTRAL_ARRANGEMENT
        );
        assert_eq!(
            word_among(None, &CREDENTIAL_PATHS, NEUTRAL_CREDENTIAL_PATH),
            NEUTRAL_CREDENTIAL_PATH
        );
        assert_eq!(
            word_among(Some("handover"), &CREDENTIAL_PATHS, NEUTRAL_CREDENTIAL_PATH),
            "handover"
        );
    }

    #[test]
    fn the_history_began_and_ended_when_the_sources_segments_did_else_when_its_items_did_else_now()
    {
        let now: DateTime<Utc> = "2026-10-01T00:00:00Z".parse().unwrap();
        let (here, credits) = (HashSet::new(), nobody());
        let with = context(&here, &credits);
        let entry = candidate_json(
            "agent_message",
            "completed",
            1,
            json!({ "text": "hi", "text_len": 2, "phase": null }),
        );
        let mut lost = Lost::default();
        let item = adapt(candidate(&entry).unwrap(), &with, &mut lost).unwrap();
        let items = Some((item.created_at, item.updated_at));
        let segments = vec![
            json!({ "started_at": "2026-01-01T00:00:00.000Z", "ended_at": "2026-01-02T00:00:00.000Z" }),
            json!({ "started_at": "2026-02-01T00:00:00.000Z", "ended_at": "2026-02-03T00:00:00.000Z" }),
        ];
        let (began, ended) = span_of(&segments, items, now);
        assert_eq!(began.to_rfc3339(), "2026-01-01T00:00:00+00:00");
        assert_eq!(ended.to_rfc3339(), "2026-02-03T00:00:00+00:00");
        let (began, ended) = span_of(&[], items, now);
        assert_eq!(began, item.created_at);
        assert_eq!(ended, item.updated_at);
        assert_eq!(span_of(&[], None, now), (now, now));
    }

    #[test]
    fn what_does_not_come_is_said_once_for_the_items_and_once_for_the_files() {
        assert!(warnings_of(&Tally::default()).is_empty());
        let mut tally = Tally::default();
        tally.left_out("marker", DROPPED_MARKER);
        tally.left_out("marker", DROPPED_MARKER);
        tally.files_dangling = 1;
        let warnings = warnings_of(&tally);
        assert_eq!(warnings.len(), 2);
        assert_eq!(warnings[0].code, "history_dropped");
        assert_eq!(warnings[0].params, Some(json!({ "count": 2 })));
        assert_eq!(warnings[1].code, "missing_file");
        assert_eq!(warnings[1].params, Some(json!({ "count": 1 })));
    }
}
