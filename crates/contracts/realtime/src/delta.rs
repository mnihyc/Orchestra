//! Text and output offsets.
//!
//! While an item of a thread is in progress, the text it grows arrives as
//! `delta` frames between the `sync` frames that carry its row. Each
//! delta says where in the field it begins (`offset`) and how long it is
//! (`len`), in the unit of that field, so that a client can tell whether
//! a delta continues what it has, repeats it, or leaves a hole. The
//! arithmetic is here: [`Applied`] is what a client keeps for each growing
//! field of each item, [`PendingDelta`] what a server keeps for a reader
//! that is behind, and [`releasable`] how much of a field a server may let
//! go while a secret could still be straddling two chunks.
//!
//! Nothing here stores an item's text. A client holds the text; these
//! types hold the number that says how much of it is right.

use crate::frames::DeltaFrame;
use orch_api_contract::threads::{Item, ItemPayload, ItemStatus, SUB_STREAM_PREFIX, ToolPayload};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The longest, in milliseconds, that the row of a growing item may be
/// behind what the server has forwarded: the server persists the growing
/// text at least this often. So a row that is shorter than what a client has
/// applied is only late, and never a sign that text was lost.
pub const PERSIST_LAG_MILLIS: i64 = 1_000;

/// What a length of a field counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeltaUnit {
    /// Unicode code points. A client written in a language whose strings
    /// are counted in other units (a browser's are in UTF-16 code units,
    /// two for a character outside the first plane) converts: it counts
    /// the code points of what it has, not `length`.
    CodePoints,
    /// Bytes of the output as the command wrote it, before it was
    /// decoded as UTF-8 with the invalid sequences replaced, which makes
    /// the text a delta carries longer or shorter than its `len`.
    Bytes,
}

/// The field of an item that a delta grows.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DeltaField {
    /// What an agent message says: `payload.text`, counted in code points.
    Text,
    /// A summary of what an agent is reasoning: `payload.summary`, counted
    /// in code points.
    Summary,
    /// What a command printed: `payload.tool.output`, counted in bytes.
    Output,
}

impl DeltaField {
    /// Every field.
    pub const ALL: [DeltaField; 3] = [Self::Text, Self::Summary, Self::Output];

    /// What `offset` and `len` of this field count.
    #[must_use]
    pub const fn unit(self) -> DeltaUnit {
        match self {
            Self::Text | Self::Summary => DeltaUnit::CodePoints,
            Self::Output => DeltaUnit::Bytes,
        }
    }

    /// How long `data` is in this field's unit, where `data` alone says
    /// it: for the fields counted in code points. The length of output
    /// counts the bytes before they were decoded, which the decoded text
    /// cannot say, so `None`.
    #[must_use]
    pub fn len_of(self, data: &str) -> Option<i64> {
        match self.unit() {
            DeltaUnit::CodePoints => i64::try_from(data.chars().count()).ok(),
            DeltaUnit::Bytes => None,
        }
    }
}

/// The field an item grows while it is in progress, and how much its row
/// holds of it: the number a client starts [`Applied`] from.
///
/// An agent message grows its text, and its row says how long that is in
/// `text_len`; a reasoning item grows its summary, counted by the same
/// member; a command grows its output, and the row says how many bytes
/// were produced in `total_len`. Other items do not grow.
#[must_use]
pub fn growing(item: &Item) -> Option<(DeltaField, i64)> {
    match &item.payload {
        ItemPayload::AgentMessage(message) => Some((DeltaField::Text, message.text_len)),
        ItemPayload::Reasoning(reasoning) => Some((DeltaField::Summary, reasoning.text_len)),
        ItemPayload::ToolCall(call) => match &call.tool {
            ToolPayload::Command { output, .. } => Some((DeltaField::Output, output.total_len)),
            _ => None,
        },
        _ => None,
    }
}

/// Whether a row with this status is the last that the item will have.
#[must_use]
pub fn is_final(status: ItemStatus) -> bool {
    status != ItemStatus::InProgress
}

/// Whether the deltas of an item are sent to a subscription. The items of
/// a sub-agent's stream arrive by their rows only, unless the client
/// subscribed with `sub_streams`.
#[must_use]
pub fn wanted(stream: &str, sub_streams: bool) -> bool {
    sub_streams || !stream.starts_with(SUB_STREAM_PREFIX)
}

/// Why a delta cannot be taken as it is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeltaError {
    /// It carries nothing, or begins before the start of the field.
    #[error("a delta begins at 0 or later and carries something")]
    Empty,
    /// Its `len` is not the length of its text in the field's unit.
    #[error("the delta says it is {said} long but its text is {held}")]
    LengthDiffers {
        /// The `len` it carries.
        said: i64,
        /// The length of its text.
        held: i64,
    },
    /// It does not begin where the text waiting ends.
    #[error("the text waiting ends at {expected} but this delta begins at {got}")]
    NotContinuous {
        /// Where the waiting text ends.
        expected: i64,
        /// Where this delta begins.
        got: i64,
    },
}

/// What a client does with a delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeltaAction {
    /// It continues the text exactly: append `data`. The count has moved
    /// to the end of the delta.
    Append,
    /// It repeats, or lies inside, what is applied: ignore it.
    Ignore,
    /// It begins after what is applied, so text is missing between:
    /// read the item (`GET /threads/{id}/items/{item_id}`), apply it as
    /// a row ([`Applied::on_row`]), and go on from there.
    Gap,
}

/// What a client does with a row of the item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RowAction {
    /// The row has at least as much as the client: its text replaces the
    /// client's. The count has moved to the row's length.
    Replace,
    /// The client has more than the row, which was persisted a moment
    /// behind what was forwarded: keep the text and the count.
    Keep,
}

/// How much of one field of one item a client has applied: where the next
/// delta has to begin. It starts from the item's row, and a client keeps
/// one for each growing field of each item it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Applied(i64);

impl Applied {
    /// The count of a field whose row holds `len` of it (`text_len`, or
    /// `total_len` for output).
    #[must_use]
    pub const fn from_row(len: i64) -> Self {
        Self(len)
    }

    /// How much is applied, in the field's unit.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    /// A delta of `len` that begins at `offset`.
    ///
    /// It is appended only when it begins exactly where the applied text
    /// ends. One that begins before is ignored, even if it reaches beyond:
    /// a server never sends one like that to a client that follows the
    /// rules, and appending the part that is new would depend on counting
    /// the delta's text in the right unit. One that begins after leaves a
    /// hole.
    pub fn on_delta(&mut self, offset: i64, len: i64) -> DeltaAction {
        if offset > self.0 {
            return DeltaAction::Gap;
        }
        if offset < self.0 || len <= 0 {
            return DeltaAction::Ignore;
        }
        match offset.checked_add(len) {
            Some(end) => {
                self.0 = end;
                DeltaAction::Append
            }
            None => DeltaAction::Ignore,
        }
    }

    /// A row of the item that holds `len` of the field: the item row of a
    /// `sync` frame, or the item read after a gap.
    ///
    /// The row replaces the text if it holds at least as much as the
    /// client has. It is persisted at most a second behind what was
    /// forwarded, so a row that is shorter than what the client has
    /// already is only late, and the client keeps its text. A row of an
    /// item that is no longer in progress ([`is_final`]) is the last, and
    /// since nothing grows after it, it is never shorter than the count.
    pub fn on_row(&mut self, len: i64) -> RowAction {
        if len >= self.0 {
            self.0 = len;
            RowAction::Replace
        } else {
            RowAction::Keep
        }
    }
}

/// What becomes of a waiting delta when a row of the same item is queued
/// behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BehindRow {
    /// The row holds everything the delta says: drop the delta.
    Drop,
    /// The row holds only the start of it. The delta goes out before the
    /// row, and what grows after that is a delta of its own: a client that
    /// had the row first would skip over text that the row does not hold.
    SendFirst,
    /// The row is older than the delta: leave the delta where it is, and
    /// let it go on growing.
    Keep,
}

/// What has grown in one field of one item since the last delta that left
/// for a reader that is behind, merged into one.
///
/// A reader whose connection has frames waiting to be taken is not sent
/// one delta behind another. What has grown is kept here, and goes as a
/// single delta when the connection can take it, so that a slow link sees
/// the text in larger steps and a little later, and loses nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDelta {
    field: DeltaField,
    offset: i64,
    len: i64,
    data: String,
}

impl PendingDelta {
    /// A chunk of `field` that begins at `offset`, is `len` long and
    /// reads `data`.
    pub fn new(field: DeltaField, offset: i64, len: i64, data: String) -> Result<Self, DeltaError> {
        if offset < 0 || len <= 0 {
            return Err(DeltaError::Empty);
        }
        if let Some(held) = field.len_of(&data)
            && held != len
        {
            return Err(DeltaError::LengthDiffers { said: len, held });
        }
        Ok(Self {
            field,
            offset,
            len,
            data,
        })
    }

    /// The same, from a frame about to be sent.
    pub fn from_frame(frame: &DeltaFrame) -> Result<Self, DeltaError> {
        Self::new(frame.field, frame.offset, frame.len, frame.data.clone())
    }

    /// The field it grows.
    #[must_use]
    pub const fn field(&self) -> DeltaField {
        self.field
    }

    /// Where it begins.
    #[must_use]
    pub const fn offset(&self) -> i64 {
        self.offset
    }

    /// How long it is, in the field's unit.
    #[must_use]
    pub const fn len(&self) -> i64 {
        self.len
    }

    /// Never true: a pending delta carries something.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len <= 0
    }

    /// Where it ends, which is where the next chunk has to begin.
    #[must_use]
    pub const fn end(&self) -> i64 {
        self.offset.saturating_add(self.len)
    }

    /// What it reads.
    #[must_use]
    pub fn data(&self) -> &str {
        &self.data
    }

    /// Takes in the chunk that grew next. It has to begin where this one
    /// ends; the text of the two is joined and the lengths added, which is
    /// right in either unit as long as each chunk was cut where a
    /// character ends.
    pub fn absorb(&mut self, offset: i64, len: i64, data: &str) -> Result<(), DeltaError> {
        if offset != self.end() {
            return Err(DeltaError::NotContinuous {
                expected: self.end(),
                got: offset,
            });
        }
        let next = Self::new(self.field, offset, len, data.to_owned())?;
        self.len = self.len.saturating_add(next.len);
        self.data.push_str(&next.data);
        Ok(())
    }

    /// What becomes of this delta when a row of the item, holding `row_len`
    /// of the field, is queued after it.
    #[must_use]
    pub const fn behind_row(&self, row_len: i64) -> BehindRow {
        if self.end() <= row_len {
            BehindRow::Drop
        } else if self.offset < row_len {
            BehindRow::SendFirst
        } else {
            BehindRow::Keep
        }
    }
}

/// How much of a field a server may let go of, persist and forward,
/// when `total` of it has been produced.
///
/// A secret that is masked may straddle two chunks, so the last
/// `longest_secret - 1` of what has been produced is held back until the
/// next chunk arrives or the item ends: of a value whose end has not yet
/// arrived, no more than that much can have. `longest_secret` is the
/// length of the longest value to mask for the thread, counted in the
/// unit of the field (code points for text and summary, bytes for
/// output), and 0 when nothing is configured. Once the item has finished
/// nothing is held back. Held-back text is not lost: it leads the next
/// delta.
#[must_use]
pub const fn releasable(total: i64, longest_secret: i64, finished: bool) -> i64 {
    if finished || longest_secret <= 1 {
        return total;
    }
    let held = longest_secret - 1;
    if held >= total { 0 } else { total - held }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::Id;
    use serde_json::json;
    use std::str::FromStr;

    fn item(kind_and_payload: serde_json::Value) -> Item {
        let mut row = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01",
            "thread_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a02",
            "segment_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a03",
            "turn_id": null, "stream": "main", "parent_item_id": null,
            "status": "in_progress", "author": null, "pinned": null, "chat_refs": 0,
            "seq_created": 12, "seq_updated": 13,
            "created_at": "2026-10-08T17:04:05.123Z", "updated_at": "2026-10-08T17:04:06.000Z"
        });
        row.as_object_mut()
            .unwrap()
            .extend(kind_and_payload.as_object().unwrap().clone());
        serde_json::from_value(row).unwrap()
    }

    // ------------------------------------------------------------ the client

    #[test]
    fn deltas_that_continue_the_text_are_appended_in_turn() {
        // The two deltas of the protocol's example: 18 characters, then 16 more.
        let mut applied = Applied::from_row(0);
        assert_eq!(applied.on_delta(0, 18), DeltaAction::Append);
        assert_eq!(applied.get(), 18);
        assert_eq!(applied.on_delta(18, 16), DeltaAction::Append);
        assert_eq!(applied.get(), 34);
        // The row that completes the item says 34, as much as is applied.
        assert_eq!(applied.on_row(34), RowAction::Replace);
        assert_eq!(applied.get(), 34);
    }

    #[test]
    fn a_delta_that_repeats_what_is_applied_is_ignored() {
        let mut applied = Applied::from_row(34);
        assert_eq!(applied.on_delta(18, 16), DeltaAction::Ignore);
        assert_eq!(applied.on_delta(0, 18), DeltaAction::Ignore);
        assert_eq!(applied.get(), 34);
    }

    #[test]
    fn a_delta_that_begins_inside_the_text_is_ignored_even_if_it_reaches_beyond() {
        let mut applied = Applied::from_row(20);
        assert_eq!(applied.on_delta(10, 30), DeltaAction::Ignore);
        assert_eq!(applied.get(), 20);
    }

    #[test]
    fn a_delta_that_begins_after_the_text_is_a_gap_and_changes_nothing() {
        let mut applied = Applied::from_row(18);
        assert_eq!(applied.on_delta(20, 5), DeltaAction::Gap);
        assert_eq!(applied.get(), 18);
        // The item is read: it holds 25, more than the client has, so it replaces the text.
        assert_eq!(applied.on_row(25), RowAction::Replace);
        assert_eq!(applied.on_delta(25, 4), DeltaAction::Append);
        assert_eq!(applied.get(), 29);
    }

    #[test]
    fn a_row_that_holds_less_than_the_client_has_is_only_late() {
        let mut applied = Applied::from_row(0);
        assert_eq!(applied.on_delta(0, 18), DeltaAction::Append);
        assert_eq!(applied.on_delta(18, 16), DeltaAction::Append);
        // The row persisted a moment ago says 18; the client has 34.
        assert_eq!(applied.on_row(18), RowAction::Keep);
        assert_eq!(applied.get(), 34);
    }

    #[test]
    fn a_row_that_holds_more_than_the_client_has_takes_over_and_the_deltas_it_covers_are_ignored() {
        let mut applied = Applied::from_row(0);
        assert_eq!(applied.on_delta(0, 10), DeltaAction::Append);
        assert_eq!(applied.on_row(40), RowAction::Replace);
        assert_eq!(applied.on_delta(10, 20), DeltaAction::Ignore);
        assert_eq!(applied.on_delta(30, 10), DeltaAction::Ignore);
        assert_eq!(applied.on_delta(40, 5), DeltaAction::Append);
    }

    #[test]
    fn a_delta_that_carries_nothing_or_overflows_changes_nothing() {
        let mut applied = Applied::from_row(5);
        assert_eq!(applied.on_delta(5, 0), DeltaAction::Ignore);
        assert_eq!(applied.on_delta(5, -3), DeltaAction::Ignore);
        assert_eq!(applied.on_delta(-1, 4), DeltaAction::Ignore);
        let mut huge = Applied::from_row(i64::MAX - 1);
        assert_eq!(huge.on_delta(i64::MAX - 1, 10), DeltaAction::Ignore);
        assert_eq!(huge.get(), i64::MAX - 1);
    }

    #[test]
    fn text_is_counted_in_code_points_and_output_in_the_bytes_it_was() {
        // One character outside the first plane is one code point, four bytes and two UTF-16 units.
        let text = "a😀b";
        assert_eq!(DeltaField::Text.len_of(text), Some(3));
        assert_eq!(DeltaField::Summary.len_of("é"), Some(1));
        assert_eq!(text.len(), 6);
        assert_eq!(text.encode_utf16().count(), 4);
        // What a command printed is counted before it was decoded, which its text cannot say.
        assert_eq!(DeltaField::Output.len_of(text), None);
        assert_eq!(DeltaField::Text.unit(), DeltaUnit::CodePoints);
        assert_eq!(DeltaField::Output.unit(), DeltaUnit::Bytes);
    }

    #[test]
    fn a_field_is_written_as_the_member_it_grows() {
        assert_eq!(
            serde_json::to_value(DeltaField::ALL).unwrap(),
            json!(["text", "summary", "output"])
        );
        assert!(serde_json::from_value::<DeltaField>(json!("tail")).is_err());
    }

    #[test]
    fn an_item_says_which_field_it_grows_and_how_much_its_row_holds() {
        let message = item(
            json!({ "kind": "agent_message", "payload": { "text": "I'll start", "text_len": 10, "phase": null } }),
        );
        assert_eq!(growing(&message), Some((DeltaField::Text, 10)));

        let reasoning = item(
            json!({ "kind": "reasoning", "payload": { "summary": "Looking", "text_len": 7, "hidden": false } }),
        );
        assert_eq!(growing(&reasoning), Some((DeltaField::Summary, 7)));

        let command = item(json!({
            "kind": "tool_call",
            "payload": {
                "tool": {
                    "kind": "command", "command": "cargo test", "cwd": "/work", "exit_code": null,
                    "output": { "inline": "running", "total_len": 7, "truncated": false, "blob": null },
                    "process_id": null, "background": false, "actions": []
                },
                "title": "Run the tests", "started_at": "2026-10-08T17:04:05.123Z", "duration_ms": null,
                "raw": { "name": "Bash", "input": { "inline": "{}", "total_len": 2, "truncated": false, "blob": null }, "output": null, "is_error": false }
            }
        }));
        assert_eq!(growing(&command), Some((DeltaField::Output, 7)));

        let approval = item(
            json!({ "kind": "approval", "payload": { "approval_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a09" } }),
        );
        assert_eq!(growing(&approval), None);
    }

    #[test]
    fn only_an_item_that_is_in_progress_is_not_final() {
        assert!(!is_final(ItemStatus::InProgress));
        for status in [
            ItemStatus::Completed,
            ItemStatus::Failed,
            ItemStatus::Cancelled,
            ItemStatus::Pending,
            ItemStatus::Approved,
        ] {
            assert!(is_final(status), "{status:?}");
        }
    }

    #[test]
    fn the_text_of_a_sub_agent_arrives_by_rows_unless_the_client_asked_for_it() {
        assert!(wanted("main", false));
        assert!(!wanted("sub:0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a0b", false));
        assert!(wanted("sub:0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a0b", true));
        assert!(wanted("main", true));
    }

    // ------------------------------------------------------------ the server

    fn pending(offset: i64, text: &str) -> PendingDelta {
        let len = i64::try_from(text.chars().count()).unwrap();
        PendingDelta::new(DeltaField::Text, offset, len, text.to_owned()).unwrap()
    }

    #[test]
    fn what_grows_while_a_reader_is_behind_goes_as_one_delta() {
        let mut waiting = pending(0, "I'll start by read");
        waiting.absorb(18, 16, "ing the handler.").unwrap();
        assert_eq!(
            (waiting.offset(), waiting.len(), waiting.end()),
            (0, 34, 34)
        );
        assert_eq!(waiting.data(), "I'll start by reading the handler.");
        // A client that is at 0 takes it whole, and is where a client that had both would be.
        let mut applied = Applied::from_row(0);
        assert_eq!(
            applied.on_delta(waiting.offset(), waiting.len()),
            DeltaAction::Append
        );
        assert_eq!(applied.get(), 34);
    }

    #[test]
    fn a_chunk_that_does_not_continue_the_waiting_text_is_refused() {
        let mut waiting = pending(10, "abc");
        assert_eq!(
            waiting.absorb(14, 1, "d"),
            Err(DeltaError::NotContinuous {
                expected: 13,
                got: 14
            })
        );
        assert_eq!(
            waiting.absorb(12, 1, "c"),
            Err(DeltaError::NotContinuous {
                expected: 13,
                got: 12
            })
        );
        assert_eq!(waiting.data(), "abc");
    }

    #[test]
    fn a_chunk_whose_length_is_not_that_of_its_text_is_refused() {
        let wrong = PendingDelta::new(DeltaField::Text, 0, 4, "a😀b".to_owned());
        assert_eq!(wrong, Err(DeltaError::LengthDiffers { said: 4, held: 3 }));
        assert!(PendingDelta::new(DeltaField::Text, 0, 3, "a😀b".to_owned()).is_ok());
        assert_eq!(
            PendingDelta::new(DeltaField::Text, 0, 0, String::new()),
            Err(DeltaError::Empty)
        );
        assert_eq!(
            PendingDelta::new(DeltaField::Text, -1, 1, "a".to_owned()),
            Err(DeltaError::Empty)
        );
        // Output is counted before decoding, so the length of its text proves nothing.
        assert!(PendingDelta::new(DeltaField::Output, 0, 3, "\u{fffd}".to_owned()).is_ok());
    }

    #[test]
    fn chunks_of_output_are_merged_by_the_bytes_they_were() {
        // Two bytes of invalid UTF-8, then ten of text: the first reads as two replacement characters.
        let mut waiting =
            PendingDelta::new(DeltaField::Output, 100, 2, "\u{fffd}\u{fffd}".to_owned()).unwrap();
        waiting.absorb(102, 10, "hello wrld").unwrap();
        assert_eq!(
            (waiting.offset(), waiting.len(), waiting.end()),
            (100, 12, 112)
        );
        assert_eq!(waiting.data(), "\u{fffd}\u{fffd}hello wrld");
    }

    #[test]
    fn a_row_that_holds_all_of_a_waiting_delta_makes_it_pointless() {
        let waiting = pending(10, "abcde");
        assert_eq!(waiting.behind_row(15), BehindRow::Drop);
        assert_eq!(waiting.behind_row(40), BehindRow::Drop);
    }

    #[test]
    fn a_row_that_holds_the_start_of_a_waiting_delta_must_come_after_it() {
        let waiting = pending(10, "abcde");
        assert_eq!(waiting.behind_row(12), BehindRow::SendFirst);
        assert_eq!(waiting.behind_row(11), BehindRow::SendFirst);
    }

    #[test]
    fn a_row_that_is_older_than_a_waiting_delta_leaves_it_to_grow() {
        let waiting = pending(10, "abcde");
        assert_eq!(waiting.behind_row(10), BehindRow::Keep);
        assert_eq!(waiting.behind_row(3), BehindRow::Keep);
    }

    #[test]
    fn a_waiting_delta_is_the_one_a_client_reads_from_the_frame() {
        let frame = DeltaFrame {
            id: 1,
            feed: format!(
                "thread:{}",
                Id::from_str("0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a0a").unwrap()
            )
            .parse()
            .unwrap(),
            item_id: Id::from_str("0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01").unwrap(),
            field: DeltaField::Text,
            offset: 18,
            len: 16,
            data: "ing the handler.".to_owned(),
        };
        let waiting = PendingDelta::from_frame(&frame).unwrap();
        assert_eq!(
            (
                waiting.field(),
                waiting.offset(),
                waiting.len(),
                waiting.data()
            ),
            (DeltaField::Text, 18, 16, "ing the handler.")
        );
        assert!(!waiting.is_empty());
    }

    // ------------------------------------------- the server and the client, on a slow link

    /// A small generator of numbers that nobody can predict from the test.
    struct Dice(u64);

    impl Dice {
        fn next(&mut self) -> u64 {
            // splitmix64: a well known generator that is a few lines long and needs no crate.
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        fn below(&mut self, bound: u64) -> u64 {
            self.next() % bound
        }
    }

    /// Characters of all widths, so that bytes, UTF-16 units and code points differ.
    const ALPHABET: [char; 8] = ['a', 'b', ' ', 'é', 'ß', '字', '😀', '\n'];

    /// A frame a slow reader is sent.
    #[derive(Debug)]
    enum Sent {
        Delta { offset: i64, len: i64, data: String },
        Row { text: String, len: i64 },
    }

    /// What a client shows of one field, and the count that goes with it.
    struct Screen {
        text: String,
        applied: Applied,
        fetches: u32,
    }

    impl Screen {
        fn new() -> Self {
            Self {
                text: String::new(),
                applied: Applied::from_row(0),
                fetches: 0,
            }
        }

        /// The invariant of the rules: the screen shows a prefix of the final text, as long as the count says.
        fn check(&self, truth: &str) {
            assert!(
                truth.starts_with(&self.text),
                "{:?} is not a prefix of {:?}",
                self.text,
                truth
            );
            assert_eq!(
                i64::try_from(self.text.chars().count()).unwrap(),
                self.applied.get()
            );
        }

        /// Takes a frame; `persisted` is what an item read would answer now.
        fn take(&mut self, frame: &Sent, persisted: &str) {
            match frame {
                Sent::Delta { offset, len, data } => match self.applied.on_delta(*offset, *len) {
                    DeltaAction::Append => self.text.push_str(data),
                    DeltaAction::Ignore => {}
                    DeltaAction::Gap => {
                        self.fetches += 1;
                        let len = i64::try_from(persisted.chars().count()).unwrap();
                        if self.applied.on_row(len) == RowAction::Replace {
                            persisted.clone_into(&mut self.text);
                        }
                    }
                },
                Sent::Row { text, len } => {
                    if self.applied.on_row(*len) == RowAction::Replace {
                        text.clone_into(&mut self.text);
                    }
                }
            }
        }
    }

    #[test]
    fn a_reader_on_a_slow_link_gets_fewer_larger_deltas_and_loses_nothing_and_never_has_a_gap() {
        for game in 0..300_u64 {
            let mut dice = Dice(game);
            // What the agent says, in chunks of one to nine characters.
            let mut truth = String::new();
            let mut forwarded = 0_i64; // how many characters have left the server's process
            let mut persisted = 0_usize; // how many characters of `truth` the row holds
            let mut waiting: Option<PendingDelta> = None;
            let mut queue: Vec<Sent> = Vec::new();
            let mut screen = Screen::new();

            for _ in 0..80 {
                // Text grows, and joins what waits, or starts it.
                let chunk: String = (0..=dice.below(8))
                    .map(|_| ALPHABET[usize::try_from(dice.below(8)).unwrap()])
                    .collect();
                let len = i64::try_from(chunk.chars().count()).unwrap();
                truth.push_str(&chunk);
                match waiting.as_mut() {
                    Some(held) => held.absorb(forwarded, len, &chunk).unwrap(),
                    None => {
                        waiting = Some(
                            PendingDelta::new(DeltaField::Text, forwarded, len, chunk).unwrap(),
                        )
                    }
                }
                forwarded += len;

                // The row is persisted behind what was forwarded, and queued when it is.
                if dice.below(4) == 0 {
                    let behind = usize::try_from(dice.below(6)).unwrap();
                    let total = truth.chars().count();
                    persisted = persisted.max(total.saturating_sub(behind));
                    let text: String = truth.chars().take(persisted).collect();
                    let row_len = i64::try_from(persisted).unwrap();
                    if let Some(held) = waiting.take() {
                        match held.behind_row(row_len) {
                            BehindRow::Drop => {}
                            BehindRow::SendFirst => queue.push(Sent::Delta {
                                offset: held.offset(),
                                len: held.len(),
                                data: held.data().to_owned(),
                            }),
                            BehindRow::Keep => waiting = Some(held),
                        }
                    }
                    queue.push(Sent::Row { text, len: row_len });
                    // What grows after a dropped or sent delta begins where the server stands.
                    // (`waiting` is then None and the next chunk starts a new one at `forwarded`.)
                }

                // The socket takes some of what is queued, and the waiting delta only when nothing is ahead of it.
                let persisted_now: String = truth.chars().take(persisted).collect();
                while !queue.is_empty() && dice.below(3) != 0 {
                    let frame = queue.remove(0);
                    screen.take(&frame, &persisted_now);
                    screen.check(&truth);
                }
                if queue.is_empty()
                    && dice.below(3) == 0
                    && let Some(held) = waiting.take()
                {
                    let frame = Sent::Delta {
                        offset: held.offset(),
                        len: held.len(),
                        data: held.data().to_owned(),
                    };
                    screen.take(&frame, &persisted_now);
                    screen.check(&truth);
                }
            }

            // The item completes: the waiting delta goes out in turn, then the final row with everything.
            if let Some(held) = waiting.take() {
                match held.behind_row(i64::try_from(truth.chars().count()).unwrap()) {
                    BehindRow::Drop => {}
                    _ => queue.push(Sent::Delta {
                        offset: held.offset(),
                        len: held.len(),
                        data: held.data().to_owned(),
                    }),
                }
            }
            queue.push(Sent::Row {
                text: truth.clone(),
                len: i64::try_from(truth.chars().count()).unwrap(),
            });
            for frame in queue.drain(..) {
                screen.take(&frame, &truth);
                screen.check(&truth);
            }
            assert_eq!(screen.text, truth, "game {game}");
            assert_eq!(
                screen.fetches, 0,
                "a gap with the rules followed, game {game}"
            );
        }
    }

    #[test]
    fn whatever_deltas_are_lost_repeated_or_swapped_the_text_is_the_persisted_text_at_completion() {
        for game in 0..300_u64 {
            let mut dice = Dice(game);
            let mut truth = String::new();
            let mut frames: Vec<Sent> = Vec::new();
            let mut forwarded = 0_i64;
            for _ in 0..40 {
                let chunk: String = (0..=dice.below(6))
                    .map(|_| ALPHABET[usize::try_from(dice.below(8)).unwrap()])
                    .collect();
                let len = i64::try_from(chunk.chars().count()).unwrap();
                truth.push_str(&chunk);
                frames.push(Sent::Delta {
                    offset: forwarded,
                    len,
                    data: chunk,
                });
                forwarded += len;
            }
            // Spoil the stream: drop some frames, repeat some, swap neighbours.
            let mut spoiled: Vec<Sent> = Vec::new();
            for frame in frames {
                let Sent::Delta { offset, len, data } = &frame else {
                    continue;
                };
                match dice.below(8) {
                    0 => {}
                    1 => {
                        spoiled.push(Sent::Delta {
                            offset: *offset,
                            len: *len,
                            data: data.clone(),
                        });
                        spoiled.push(frame);
                    }
                    2 if !spoiled.is_empty() => {
                        let last = spoiled.pop().unwrap();
                        spoiled.push(frame);
                        spoiled.push(last);
                    }
                    _ => spoiled.push(frame),
                }
            }
            let mut screen = Screen::new();
            for frame in &spoiled {
                // An item read answers what was persisted: a little behind the frame being taken.
                let upto = frame_end(frame).saturating_sub(i64::try_from(dice.below(4)).unwrap());
                let persisted: String = truth
                    .chars()
                    .take(usize::try_from(upto.max(0)).unwrap())
                    .collect();
                screen.take(frame, &persisted);
                screen.check(&truth);
            }
            // The final row.
            let last = Sent::Row {
                text: truth.clone(),
                len: i64::try_from(truth.chars().count()).unwrap(),
            };
            screen.take(&last, &truth);
            assert_eq!(screen.text, truth, "game {game}");
        }
    }

    fn frame_end(frame: &Sent) -> i64 {
        match frame {
            Sent::Delta { offset, len, .. } => offset + len,
            Sent::Row { len, .. } => *len,
        }
    }

    // ------------------------------------------------------------ redaction

    #[test]
    fn the_last_characters_of_a_secrets_length_less_one_are_held_back_until_the_item_ends() {
        // Nothing configured: nothing is held. A secret of one character cannot straddle a boundary.
        assert_eq!(releasable(100, 0, false), 100);
        assert_eq!(releasable(100, 1, false), 100);
        // A secret of 8 may have 7 of its characters at the end of what has been produced.
        assert_eq!(releasable(100, 8, false), 93);
        assert_eq!(releasable(5, 8, false), 0);
        assert_eq!(releasable(7, 8, false), 0);
        assert_eq!(releasable(8, 8, false), 1);
        // When the item has ended everything goes.
        assert_eq!(releasable(100, 8, true), 100);
        assert_eq!(releasable(0, 8, false), 0);
    }

    /// Masks every complete occurrence of `secret`, as a server does to what it holds.
    fn masked(text: &str, secret: &str) -> String {
        text.replace(secret, "<hidden>")
    }

    #[test]
    fn a_secret_cut_by_the_end_of_a_chunk_is_never_released_in_part() {
        // The value to mask is 10 long. The text has it twice and begins it a third time at the end,
        // and arrives in chunks of every size, so that it is cut everywhere.
        let secret = "s3cr3t-key";
        let longest = i64::try_from(secret.chars().count()).unwrap();
        let produced =
            format!("The key is {secret}, again {secret}, and a third that stops: s3cr3t-k");
        let whole = masked(&produced, secret);
        let chars: Vec<char> = produced.chars().collect();
        for size in 1..=chars.len() {
            let mut total = 0;
            let mut released = String::new();
            while total < chars.len() {
                total = (total + size).min(chars.len());
                let finished = total == chars.len();
                // What the server holds is masked; of that, the part that cannot change goes.
                let held: Vec<char> = masked(&chars[..total].iter().collect::<String>(), secret)
                    .chars()
                    .collect();
                let may = usize::try_from(releasable(
                    i64::try_from(held.len()).unwrap(),
                    longest,
                    finished,
                ))
                .unwrap();
                // What has gone is never taken back, even when masking made what is held shorter.
                let upto = may.max(released.chars().count());
                let now: String = held[..upto].iter().collect();
                // Every frame shows a prefix of what the item finally says, so no part of a secret is ever
                // seen that was masked afterwards.
                assert!(
                    whole.starts_with(&now),
                    "chunks of {size}: {now:?} is not the start of {whole:?}"
                );
                assert!(
                    now.starts_with(&released),
                    "chunks of {size}: text that had gone changed"
                );
                released = now;
            }
            assert_eq!(released, whole, "chunks of {size}");
        }
    }
}
