//! The history of a thread as it travels to another agent: the turns asked for, what each method keeps, converts and drops, the neutral archive that an adapter imports, and the transcript a summary hands to an agent that is to read it.
//!
//! A history is the items of the main stream that were made in the turns a request names. The method that
//! carries it ([`HistoryMethod`]) decides what becomes of each kind of item; [`report_counts`] says so before the
//! job runs, from [`Counts`] that SQL made, and the adapter's own report replaces it once the session is open.
//! **Nothing here holds a history.** A plan counts one; a carry takes the newest turns that fit a bound, read from
//! the end a page at a time ([`newest`], `cut::recent`); a transcript is written to a file a page of turns at a
//! time (`cut::transcript_file`). What they cost follows the bound or the page, never the length of the history.
//!
//! **The neutral archive** is one JSON object to a line, oldest first: `kind`, `status`, `stream`,
//! `turn_ordinal`, the adapter's `native_ref` where the item had one, and the item's `payload` as the API
//! shows it. Only what a model saw is in it: what people said, what the agent said, the steps of its work
//! and its plan. Reasoning, approvals, announcements, the thread's own history lines and errors are counted
//! as dropped, and a range that does not begin at the first turn opens with a line that says which turns
//! were left out.
//!
//! A file handoff writes the full conversation separately. A summary handoff asks the configured
//! producer for bounded, attributed text beside that file. [`Handoff`] is the producer boundary;
//! a missing or failed producer is refused, without disguising a transcript as a summary.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::history_pages::{Backward, Source};
use crate::written::{self, Detail};
use async_trait::async_trait;
use orch_api_contract::forks::{Fidelity, HistoryMethod, HistoryReport, ReportDropped, ReportKind};
use orch_db::queries::threads::ItemRow;
use orch_types::{Id, Json};
use std::collections::BTreeMap;
use std::sync::Arc;

pub(crate) mod cut;

/// The directory of the new agent's `.orch` in which the transcript is written.
pub const HANDOFF_DIRECTORY: &str = "handoff";

/// The kinds of item that are part of what a model saw.
const SEEN_BY_THE_MODEL: [&str; 4] = ["user_message", "agent_message", "tool_call", "plan"];

/// The kinds that a neutral import carries as they are.
const KEPT_AS_THEY_ARE: [&str; 2] = ["user_message", "agent_message"];

/// The kinds a neutral import writes as part of another message: the steps of a turn become one message
/// before its last answer, and the plan a checklist in it.
const CONVERTED: [&str; 2] = ["tool_call", "plan"];

/// Why the other kinds of item are not carried by a neutral import.
fn reason_not_carried(kind: &str) -> &'static str {
    match kind {
        "reasoning" => "the agent's reasoning is not carried to another agent",
        "approval" => "what was asked of people is not part of what the model saw",
        "announcement" => "an announcement is delivered once and not repeated",
        "marker" => "the lines of the thread's history are not part of the conversation",
        "error" => "an error is not part of what the model saw",
        _ => "this kind of item is not part of what the model saw",
    }
}

/// A bounded part of a history: the turns of it that were taken and the items they hold ([`newest`] takes the newest turns
/// that fit a bound; nothing here is a whole history).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selected {
    /// The turns, oldest first: the identifier and the number of each.
    pub turns: Vec<(Id, i32)>,
    /// The items of the main stream that were made in them, oldest first.
    pub items: Vec<ItemRow>,
    /// The first turn the thread has, when the history begins later: those before it are left out.
    pub left_out_before: Option<i32>,
}

/// Makes the selected, already recorded conversation readable in a fork before its first input.
/// All work uses the caller's transaction; no harness, file or second connection is awaited here.
/// With no Orch turn, only the session's adopted earlier items can be copied. A Nothing fork skips this call.
pub async fn copy_conversation(
    unit: &mut crate::unit::Unit<'_>,
    source: Id,
    target: Id,
    segment: Id,
    range: Option<(i32, i32)>,
    source_segment: Option<Id>,
) -> Result<(), EngineError> {
    if source == target {
        return Err(EngineError::unreadable(
            "A full fork cannot copy into its source execution.",
        ));
    }
    // This synchronous legacy entry point has no external wait. The confirmed job
    // path uses its already-owned durable cut instead. Keep the source lock through
    // both capture and copy, then release this temporary target-owned cut atomically.
    let source_row = orch_db::queries::threads::lock(unit.conn, source)
        .await?
        .ok_or_else(|| EngineError::unreadable("The source conversation is unavailable."))?;
    if source_segment.is_some_and(|segment| Some(segment) != source_row.current_segment_id) {
        return Err(EngineError::unreadable(
            "The requested source segment is no longer selected.",
        ));
    }
    let at = unit.now();
    let captured = orch_db::queries::thread_history_cuts::capture(
        unit.conn,
        &orch_db::queries::thread_history_cuts::Capture {
            id: target,
            thread: source,
            view: source_row.active_continuation_id,
            job: None,
            owner_thread: Some(target),
            segment: source_row.current_segment_id,
            execution_revision: source_row.execution_revision,
            range,
            at,
        },
    )
    .await?;
    cut::copy(unit, &captured, target, segment, source_segment).await?;
    sqlx::query("DELETE FROM thread_history_cuts WHERE id=$1 AND owner_thread_id=$1")
        .bind(target)
        .execute(&mut *unit.conn)
        .await
        .map_err(orch_db::DbError::from)?;
    Ok(())
}

/// What a history holds, counted: how many turns, and its items by kind. The plan counts a history in SQL and never loads
/// it ([`orch_db::queries::thread_history::range_counts`]); a report is made from this.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    /// How many turns.
    pub turns: i64,
    /// The items of the main stream by kind.
    pub items: BTreeMap<String, i64>,
    /// The first turn the thread has, when the history begins later: those before it are left out.
    pub left_out_before: Option<i32>,
    /// How much room the payloads of those items take as the database keeps them: what a reader that loads them all is
    /// measured against ([`native_proof_limit`]). Nought where it was not measured.
    pub bytes: i64,
}

/// The default of the setting `thread.native_fork_check_bytes`: the most a history may take, as the database keeps its words, for
/// a native fork point to be proved against it. It is the largest record of a session that an adapter writes, 32 MiB. The proof
/// reads the whole record and, beside it, every item of the history it is made for, and matches them one to one; a history that
/// takes more than a record can hold cannot be matched to a complete one, so no proof is begun for it, nothing of it is read, and
/// it travels as the newest turns that fit.
pub(crate) const NATIVE_PROOF_DEFAULT_BYTES: i64 =
    orch_agent_contract::discovery::RECORD_MAX_BYTES as i64;

/// How many items a history may have for a proof to be worth beginning when it may take `bytes`: a record entry is never shorter
/// than 100 bytes, so a record of that size cannot hold more, and a history with more is not worth measuring.
pub(crate) const fn native_proof_items(bytes: i64) -> i64 {
    bytes / 100
}

/// The most a history may take for a native fork point to be proved against it at `place`: the setting
/// `thread.native_fork_check_bytes` there, or its default ([`NATIVE_PROOF_DEFAULT_BYTES`]).
pub(crate) async fn native_proof_limit(
    conn: &mut orch_db::Conn,
    place: Id,
) -> Result<i64, EngineError> {
    Ok(Box::pin(orch_settings::get::<i64>(
        conn,
        orch_settings::Place::Node(place),
        orch_settings_contract::registry::keys::THREAD_NATIVE_FORK_CHECK_BYTES,
    ))
    .await?
    .unwrap_or(NATIVE_PROOF_DEFAULT_BYTES)
    .max(1))
}

impl Counts {
    /// How many items there are.
    #[must_use]
    pub fn items_total(&self) -> i64 {
        self.items.values().sum()
    }
}

/// What a method does with the items of a history, counted by kind. `carried` says what a handoff in writing puts
/// into the new agent's context ([`carried_words`]); a general sentence stands where it is not known.
#[must_use]
pub fn report(method: HistoryMethod, selected: &Selected, carried: Option<&str>) -> HistoryReport {
    let mut items: BTreeMap<String, i64> = BTreeMap::new();
    for item in &selected.items {
        *items.entry(item.kind.clone()).or_default() += 1;
    }
    report_counts(
        method,
        &Counts {
            turns: i64::try_from(selected.turns.len()).unwrap_or(i64::MAX),
            items,
            left_out_before: selected.left_out_before,
            bytes: 0,
        },
        carried,
    )
}

/// The same from counts.
#[must_use]
pub fn report_counts(
    method: HistoryMethod,
    counts: &Counts,
    carried: Option<&str>,
) -> HistoryReport {
    let mut by_kind: BTreeMap<String, ReportKind> = BTreeMap::new();
    let mut dropped: BTreeMap<(String, &'static str), i64> = BTreeMap::new();
    let mut warnings: Vec<String> = Vec::new();
    for (kind, count) in &counts.items {
        let (kind, count) = (kind.as_str(), *count);
        let entry = by_kind.entry(kind.to_owned()).or_insert(ReportKind {
            kept: 0,
            converted: 0,
            dropped: 0,
        });
        match method {
            HistoryMethod::Nothing => entry.dropped += count,
            HistoryMethod::SameSession
            | HistoryMethod::NativeFork
            | HistoryMethod::NativeTransplant => {
                entry.kept += count;
            }
            HistoryMethod::NeutralImport => {
                if KEPT_AS_THEY_ARE.contains(&kind) {
                    entry.kept += count;
                } else if CONVERTED.contains(&kind) {
                    entry.converted += count;
                } else {
                    entry.dropped += count;
                    *dropped
                        .entry((kind.to_owned(), reason_not_carried(kind)))
                        .or_default() += count;
                }
            }
            HistoryMethod::SummaryHandoff | HistoryMethod::FileHandoff => {
                if kind == "reasoning" {
                    entry.dropped += count;
                    *dropped
                        .entry((kind.to_owned(), "the agent's reasoning is never written"))
                        .or_default() += count;
                } else {
                    entry.converted += count;
                }
            }
        }
    }
    if let Some(first) = counts.left_out_before {
        warnings.push(format!(
            "earlier conversation is left out; this range begins at turn {first}"
        ));
    }
    match method {
        HistoryMethod::Nothing => warnings
            .push("no history travels: the new session knows nothing of the source".to_owned()),
        HistoryMethod::FileHandoff => warnings.push(carried.map_or_else(
            || "the new agent is given the most recent turns that fit in its context, word for word, and the whole conversation as a file to read".to_owned(),
            str::to_owned,
        )),
        HistoryMethod::NeutralImport => warnings.extend(carried.map(str::to_owned)),
        HistoryMethod::SummaryHandoff => warnings.push(
            "the new agent is given a summary, or the last turns briefly where none is written, and the whole conversation as a file to read"
                .to_owned(),
        ),
        _ => {}
    }
    HistoryReport {
        fidelity: match method {
            HistoryMethod::NeutralImport | HistoryMethod::FileHandoff => Fidelity::Structured,
            HistoryMethod::SummaryHandoff => Fidelity::Summary,
            _ => Fidelity::Lossless,
        },
        items_total: counts.items_total(),
        by_kind,
        dropped: dropped
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

/// How much of a conversation a neutral import takes where the new model's window is not known: 256 KiB, about 85,000
/// tokens, which fits every window in use. A conversation that is longer travels as the file handoff, whose recent turns
/// are few and brief where the window is not known.
pub(crate) const UNKNOWN_WINDOW_NEUTRAL_BYTES: usize = 256 * 1024;

/// What a neutral import puts into the new agent's context, as a plan says it: the turns told as items, and how much of
/// the new model's window they fill.
#[must_use]
pub(crate) fn neutral_words(bytes: usize, window: Option<i64>) -> String {
    let tokens = i64::try_from(bytes).unwrap_or(i64::MAX) / CARRY_BYTES_PER_TOKEN;
    match window {
        Some(window) => format!(
            "the new agent is told the conversation as items, about {tokens} tokens of its {window}-token window"
        ),
        None => format!(
            "the new agent is told the conversation as items, about {tokens} tokens; the new model's window is not known"
        ),
    }
}

/// A cautious count of bytes for one token of carried text: English runs near four bytes a token and Chinese
/// near two to three, so three keeps a carry inside its share of the window for either.
pub(crate) const CARRY_BYTES_PER_TOKEN: i64 = 3;

/// What a handoff in writing puts into the new agent's context beside the file, as the plan says it: the most recent
/// turns up to the carry budget of a model whose window is known ([`carry_budget`]), otherwise the last turns briefly.
#[must_use]
pub(crate) fn carried_words(budget: Option<usize>, window: Option<i64>) -> String {
    match (budget, window) {
        (Some(budget), Some(window)) => format!(
            "the new agent is given the most recent turns word for word, up to about {} tokens of its {window}-token window, and the whole conversation as a file to read",
            i64::try_from(budget).unwrap_or(i64::MAX) / CARRY_BYTES_PER_TOKEN
        ),
        _ => "the new model's window is not known: the new agent is given the last 3 turns briefly and the whole conversation as a file to read".to_owned(),
    }
}

/// The window, in tokens, that a model's entry on its provider says the harness works with.
#[must_use]
pub(crate) fn model_window(parameters: &Json) -> Option<i64> {
    parameters
        .get(orch_agent_contract::parameters::MODEL_WINDOW_KEY)
        .and_then(Json::as_i64)
        .filter(|window| *window > 0)
}

/// How many bytes of the conversation a switch may carry into the new agent's context at `place`: the place's
/// share of the new model's `window` (`thread.switch_carry_share`), the rest kept for work. None where the
/// window is not known.
pub(crate) async fn carry_budget(
    conn: &mut orch_db::Conn,
    place: Id,
    window: Option<i64>,
) -> Result<Option<usize>, EngineError> {
    let Some(window) = window else {
        return Ok(None);
    };
    let share = Box::pin(orch_settings::get::<i64>(
        conn,
        orch_settings::Place::Node(place),
        orch_settings_contract::registry::keys::THREAD_SWITCH_CARRY_SHARE,
    ))
    .await?
    .unwrap_or(80)
    .clamp(10, 95);
    Ok(usize::try_from(window.saturating_mul(share) / 100 * CARRY_BYTES_PER_TOKEN).ok())
}

/// The line that opens a neutral archive of a range that begins at the turn `first`, later than the first turn.
fn left_out_line(first: i32) -> Json {
    serde_json::json!({
        "kind": "user_message", "status": "completed", "stream": "main", "turn_ordinal": 0,
        "payload": {
            "parts": [ { "type": "text", "text": format!(
                "Earlier conversation is not included; this range begins at turn {first}."
            ) } ],
            "input_id": Id::nil(), "delivery": "queue", "source": "system"
        }
    })
}

/// Whether a model saw the item, and so whether the neutral archive holds it.
pub(crate) fn is_seen(item: &ItemRow) -> bool {
    SEEN_BY_THE_MODEL.contains(&item.kind.as_str())
}

/// One item as a line of the neutral archive, made in the turn numbered `ordinal` (0 for a line that belongs to none).
pub(crate) fn neutral_line(item: &ItemRow, ordinal: i32) -> String {
    let mut line = serde_json::json!({
        "kind": item.kind, "status": item.status, "stream": item.stream,
        "turn_ordinal": ordinal, "payload": item.payload,
    });
    if let (Some(object), Some(native)) = (line.as_object_mut(), item.native_ref.as_ref()) {
        object.insert("native_ref".to_owned(), Json::String(native.clone()));
    }
    line.to_string()
}

/// The neutral archive of a history: one line to an item that a model saw, oldest first.
#[must_use]
pub fn neutral_lines(selected: &Selected) -> String {
    let ordinals: BTreeMap<Id, i32> = selected.turns.iter().copied().collect();
    let mut text = String::new();
    if let Some(first) = selected.left_out_before {
        text.push_str(&left_out_line(first).to_string());
        text.push('\n');
    }
    for item in selected.items.iter().filter(|item| is_seen(item)) {
        let ordinal = item
            .turn_id
            .and_then(|turn| ordinals.get(&turn).copied())
            .unwrap_or(0);
        text.push_str(&neutral_line(item, ordinal));
        text.push('\n');
    }
    text
}

/// How many turns are asked for at a time when a history is read from its end.
const NEWEST_PAGE_TURNS: usize = 25;

/// The newest turns of a history whose neutral archive fits a bound.
pub(crate) struct Newest {
    /// The turns taken and their items, ready for [`neutral_lines`].
    pub selected: Selected,
    /// How long their neutral archive is.
    pub bytes: usize,
    /// Whether nothing of the range is left out.
    pub complete: bool,
    /// What reading them took.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "the size tests read what a carry cost; the code only keeps it"
        )
    )]
    pub(crate) reads: crate::history_pages::Reads,
}

/// Takes the newest turns of the turns numbered `range` (of `last` that the history has) whose neutral archive, with the
/// line that says where it begins, fits in `budget` bytes. It reads from the end a page at a time and stops at the first
/// turn that does not fit, so what it costs follows the bound and not the length of the history. A turn is never cut: one
/// that alone is larger than the bound is left out, and the others with it.
pub(crate) async fn newest(
    conn: &mut orch_db::Conn,
    source: Source<'_>,
    range: (i64, i64),
    last: i64,
    budget: usize,
) -> Result<Newest, EngineError> {
    let trailing = matches!(source, Source::Live { .. });
    // Reserve the opening line before assembling any turn. An oversized newest turn can otherwise accumulate an
    // entire history in the reader before the exact same whole-turn refusal is reached below.
    let reserved = left_out_line(i32::MAX).to_string().len() + 1;
    let mut reader = Backward::between(source, conn, range, last, trailing)
        .await?
        .with_neutral_bound(budget.saturating_sub(reserved));
    let mut taken: Vec<(Id, i32)> = Vec::new();
    let mut items: Vec<ItemRow> = Vec::new();
    let mut used = 0usize;
    let mut complete = true;
    'read: loop {
        let page = reader.next(conn, NEWEST_PAGE_TURNS).await?;
        if page.is_empty() {
            break;
        }
        for bundle in page {
            let ordinal = bundle.turn.ordinal;
            let kept: Vec<ItemRow> = bundle
                .items
                .into_iter()
                .filter(|item| (item.turn_id.is_some() || item.before_orch) && is_seen(item))
                .collect();
            let bytes: usize = kept
                .iter()
                .map(|item| {
                    neutral_line(item, if item.turn_id.is_some() { ordinal } else { 0 }).len() + 1
                })
                .sum();
            if used + bytes + reserved > budget {
                complete = false;
                break 'read;
            }
            used += bytes;
            taken.push((bundle.turn.id, ordinal));
            items.extend(kept);
        }
    }
    complete &= !reader.neutral_overflowed();
    if last == 0 && range.0 <= 1 {
        // A history of lines from before Orch alone has no turn to read by: its lines are read from the end.
        let mut before = None;
        'loose: loop {
            let page = source.items_before(conn, before).await?;
            let Some(first) = page.first().map(|item| item.seq_created) else {
                break;
            };
            before = Some(first);
            for item in page
                .into_iter()
                .rev()
                .filter(|item| item.before_orch && is_seen(item))
            {
                let bytes = neutral_line(&item, 0).len() + 1;
                if used + bytes + reserved > budget {
                    complete = false;
                    break 'loose;
                }
                used += bytes;
                items.push(item);
            }
        }
    }
    taken.reverse();
    items.sort_by_key(|item| item.seq_created);
    let first = taken.first().map(|(_, ordinal)| *ordinal);
    let left_out_before = match first {
        Some(first) if first > 1 => Some(first),
        None if !complete => i32::try_from(last + 1).ok(),
        _ => (range.0 > 1).then(|| i32::try_from(range.0).ok()).flatten(),
    };
    let bytes = used
        + if left_out_before.is_some() {
            reserved
        } else {
            0
        };
    Ok(Newest {
        selected: Selected {
            turns: taken,
            items,
            left_out_before,
        },
        bytes,
        complete,
        reads: reader.reads(),
    })
}

/// The path of the transcript of `thread`, relative to the `.orch` directory it is written under.
#[must_use]
pub fn handoff_path(thread: Id) -> String {
    format!("{HANDOFF_DIRECTORY}/{thread}.md")
}

/// What produces the text that a summary handoff writes for the new agent.
#[async_trait]
pub trait Handoff: Send + Sync {
    /// The text for the agent that continues `thread`, through its turn `through` (the last completed
    /// turn when none).
    async fn text(
        &self,
        engine: &Engine,
        thread: Id,
        through: Option<i64>,
    ) -> Result<String, EngineError>;
}

/// How much of the conversation the transcript renderer gives where a summary is wanted: the newest turns that fit in
/// 64 KiB, a page that a person or a model reads at once. The whole conversation is the file beside it, written a page
/// at a time and never held.
const TRANSCRIPT_STAND_IN_BYTES: usize = 64 * 1024;

/// The transcript renderer that stands in for a summary: the newest turns of the conversation, within a bound.
#[derive(Debug, Clone, Copy, Default)]
pub struct Transcript;

#[async_trait]
impl Handoff for Transcript {
    async fn text(
        &self,
        engine: &Engine,
        thread: Id,
        through: Option<i64>,
    ) -> Result<String, EngineError> {
        let mut conn = engine.db().acquire().await?;
        let read = written::read_conversation(
            &mut conn,
            thread,
            (None, through),
            Detail::Full,
            Some(TRANSCRIPT_STAND_IN_BYTES),
        )
        .await?
        .ok_or_else(|| EngineError::unreadable("the thread to write down is not there"))?;
        Ok(read.text)
    }
}

/// The configured summary producer; an unavailable producer refuses the requested summary.
#[derive(Debug, Clone, Copy, Default)]
pub struct Configured;

#[async_trait]
impl Handoff for Configured {
    async fn text(
        &self,
        engine: &Engine,
        thread: Id,
        through: Option<i64>,
    ) -> Result<String, EngineError> {
        crate::summaries::handoff(engine, thread, through, orch_types::new_id()).await
    }
}

impl Engine {
    /// Attaches what writes the text of a summary handoff, in place of the transcript.
    pub fn attach_handoff(&self, handoff: Arc<dyn Handoff>) {
        *self
            .inner
            .handoff
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(handoff);
    }

    /// What writes a summary handoff: the attached producer, else the configured one.
    #[must_use]
    pub fn handoff(&self) -> Arc<dyn Handoff> {
        self.inner
            .handoff
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| Arc::new(Configured))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn item(kind: &str, turn: Id) -> ItemRow {
        ItemRow {
            continuation_id: Id::nil(),
            id: orch_types::new_id(),
            thread_id: Id::nil(),
            segment_id: Id::nil(),
            turn_id: Some(turn),
            model_input_id: None,
            stream: "main".to_owned(),
            parent_item_id: None,
            kind: kind.to_owned(),
            status: "completed".to_owned(),
            author_id: None,
            payload: serde_json::json!({ "text": kind }),
            native_ref: None,
            before_orch: false,
            search_text: None,
            pinned_at: None,
            pinned_by: None,
            chat_refs: 0,
            seq_created: 1,
            seq_updated: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn history() -> Selected {
        let turn = orch_types::new_id();
        Selected {
            turns: vec![(turn, 3)],
            items: [
                "user_message",
                "reasoning",
                "tool_call",
                "plan",
                "agent_message",
                "marker",
            ]
            .iter()
            .map(|kind| item(kind, turn))
            .collect(),
            left_out_before: Some(3),
        }
    }

    #[test]
    fn a_neutral_import_keeps_what_was_said_converts_the_steps_and_drops_the_rest() {
        let report = report(HistoryMethod::NeutralImport, &history(), None);
        assert_eq!(report.fidelity, Fidelity::Structured);
        assert_eq!(report.items_total, 6);
        assert_eq!(report.by_kind["agent_message"].kept, 1);
        assert_eq!(report.by_kind["tool_call"].converted, 1);
        assert_eq!(report.by_kind["plan"].converted, 1);
        assert_eq!(report.by_kind["reasoning"].dropped, 1);
        assert_eq!(report.by_kind["marker"].dropped, 1);
        assert_eq!(report.dropped.len(), 2);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("left out")),
            "a history that does not begin at the first turn says so"
        );
    }

    #[test]
    fn a_native_method_keeps_everything_and_no_history_keeps_nothing() {
        let all = report(HistoryMethod::NativeTransplant, &history(), None);
        assert_eq!(all.fidelity, Fidelity::Lossless);
        assert!(all.by_kind.values().all(|counts| counts.dropped == 0));
        let none = report(HistoryMethod::Nothing, &history(), None);
        assert!(none.by_kind.values().all(|counts| counts.kept == 0));
        assert!(
            none.warnings
                .iter()
                .any(|warning| warning.contains("no history"))
        );
    }

    #[test]
    fn the_neutral_archive_holds_what_the_model_saw_and_opens_with_the_turns_left_out() {
        let text = neutral_lines(&history());
        let lines: Vec<Json> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let kinds: Vec<&str> = lines
            .iter()
            .map(|line| line["kind"].as_str().unwrap())
            .collect();
        assert_eq!(
            kinds,
            [
                "user_message",
                "user_message",
                "tool_call",
                "plan",
                "agent_message"
            ],
            "the opening line, then the four kinds a model saw; reasoning and the history line are left out"
        );
        assert!(
            lines[0]["payload"]["parts"][0]["text"]
                .as_str()
                .unwrap()
                .contains("begins at turn 3")
        );
        assert_eq!(lines[2]["turn_ordinal"], 3);
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn the_notice_names_the_file_and_says_what_of_the_conversation_is_in_the_context() {
        use crate::summaries::{Carried, handoff_notice};
        let thread = orch_types::new_id();
        let at = orch_types::Time::new(chrono::Utc::now());
        let file = format!(".orch/handoff/{thread}.md");
        assert_eq!(handoff_path(thread), format!("handoff/{thread}.md"));
        let nothing = handoff_notice(thread, 5, 900, None, None);
        assert!(
            nothing.contains(&file)
                && nothing.contains("None of its conversation is in your context")
        );
        let recent = Carried {
            from: 4,
            to: 5,
            brief: false,
            text: "Turn 4 words".to_owned(),
        };
        let carried = handoff_notice(thread, 5, 900, None, Some((&recent, at)));
        assert!(
            carried.contains("turns, 4 to 5 of 5, follow word for word")
                && carried.contains("Turn 4 words")
                && carried.contains(&file),
            "{carried}"
        );
        let whole = Carried {
            from: 1,
            ..recent.clone()
        };
        let whole = handoff_notice(thread, 5, 900, None, Some((&whole, at)));
        assert!(
            whole.contains("Its whole conversation, 5 turns, follows word for word"),
            "{whole}"
        );
        let brief = Carried {
            from: 3,
            brief: true,
            ..recent
        };
        let brief = handoff_notice(thread, 5, 900, None, Some((&brief, at)));
        assert!(
            brief.contains("only turns 3 to 5 of 5 are in your context"),
            "{brief}"
        );
        let summary = handoff_notice(thread, 5, 900, Some(("The work stands here.", at)), None);
        assert!(
            summary.contains("A summary of it follows")
                && summary.contains("The work stands here.")
        );
    }
}
