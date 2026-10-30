//! Text that grows, held in memory and written to its item at most once an interval.
//!
//! A harness streams the text of an item a few characters at a time. Every piece reaches the people who have the
//! thread open at once ([`crate::live`]). Writing each piece to its item rewrote the whole item, a new version of
//! its row with an entry in each of its indexes, and ran its triggers, for every few characters: the square of the
//! length of a long answer, and hundreds of gigabytes written a day where many threads stream at once. So the pieces
//! of an item are held here, and what the item gained is written to it once the oldest of them has waited
//! `[engine] stream_flush_ms` (a second), and in the transaction of anything else its session says (the item's
//! update or end, the end of its turn), so that what is read of an item there is all it was told. A reader that
//! loads the item meanwhile finds its text as it was up to an interval before and the pieces since on its feed; one
//! that missed a piece reads the item again, as it always did, and a row never takes away text a reader holds.
//!
//! What is held is lost with the process that holds it: up to an interval of an item that never ends, since an item
//! that ends carries its whole text. An interval of zero writes each piece as it comes.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::ingest::{OUTPUT_INLINE_MAX_BYTES, code_points, tail_of};
use crate::unit::Unit;
use orch_db::queries::threads::{self, ItemChange, ItemRow};
use orch_link_contract::events::{DeltaField, ItemDelta};
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// An item that grows, by its session and the harness's name for it.
pub(crate) type Key = (Id, String);

/// How far each field of an item reaches once what was held of it is written.
pub(crate) type Reached = Vec<(DeltaField, i64)>;

/// The pieces of one item that are not written to it yet.
#[derive(Debug, Clone)]
pub(crate) struct Held {
    /// The item.
    pub(crate) item: Id,
    /// Its thread.
    pub(crate) thread: Id,
    /// The segment of the session that streams it (an item a fork carried is in the segment it was copied from).
    pub(crate) segment: Id,
    /// Its turn.
    pub(crate) turn: Option<Id>,
    /// The line of the conversation it is in.
    pub(crate) stream: String,
    parts: Vec<Part>,
    /// When the oldest piece that is not written yet came.
    since: Option<Instant>,
}

/// What is held of one field of an item.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Part {
    field: DeltaField,
    /// How long the field is in the item, as last read or written, in the field's unit.
    base: i64,
    /// How much came after that, in the same unit.
    len: i64,
    /// What came: all of it for text and a summary; for a command's output, the end of it that an item keeps.
    data: String,
}

impl Part {
    /// What came after the first `skip` units of what is held.
    fn after(&self, skip: i64) -> String {
        match self.field {
            DeltaField::Output => {
                let wanted = usize::try_from(self.len - skip).unwrap_or(0);
                tail_of(&self.data, wanted).to_owned()
            }
            DeltaField::Text | DeltaField::Summary => self
                .data
                .chars()
                .skip(usize::try_from(skip).unwrap_or(0))
                .collect(),
        }
    }

    /// Takes in a piece that begins where what is held ends.
    fn add(&mut self, data: &str, len: i64) {
        self.data.push_str(data);
        if self.field == DeltaField::Output {
            self.data = tail_of(&self.data, OUTPUT_INLINE_MAX_BYTES).to_owned();
        }
        self.len += len;
    }

    /// The item now reaches `reached`: what is held before that is let go.
    fn reached(&mut self, reached: i64) {
        let consumed = (reached - self.base).clamp(0, self.len);
        self.data = self.after(consumed);
        self.base += consumed;
        self.len -= consumed;
    }
}

/// How long `field` of an item's payload is, in the field's unit: code points of its text or summary, bytes of a
/// command's output.
pub(crate) fn length_of(payload: &Json, field: DeltaField) -> i64 {
    match field {
        DeltaField::Text => code_points(
            payload
                .get("text")
                .and_then(Json::as_str)
                .unwrap_or_default(),
        ),
        DeltaField::Summary => code_points(
            payload
                .get("summary")
                .and_then(Json::as_str)
                .unwrap_or_default(),
        ),
        DeltaField::Output => payload
            .pointer("/tool/output/total_len")
            .and_then(Json::as_i64)
            .unwrap_or(0),
    }
}

/// Adds `data`, `len` long in the field's unit, to the end of `field` of an item's payload, as a piece of it would.
pub(crate) fn append(payload: &mut Json, field: DeltaField, data: &str, len: i64) {
    match field {
        DeltaField::Text | DeltaField::Summary => {
            let member = if field == DeltaField::Text {
                "text"
            } else {
                "summary"
            };
            let grown = format!(
                "{}{data}",
                payload
                    .get(member)
                    .and_then(Json::as_str)
                    .unwrap_or_default()
            );
            payload["text_len"] = json!(code_points(&grown));
            payload[member] = json!(grown);
        }
        DeltaField::Output => {
            let output = payload
                .pointer("/tool/output")
                .cloned()
                .unwrap_or(Json::Null);
            let total = output.get("total_len").and_then(Json::as_i64).unwrap_or(0);
            let joined = format!(
                "{}{data}",
                output
                    .get("inline")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
            );
            let kept = tail_of(&joined, OUTPUT_INLINE_MAX_BYTES).to_owned();
            let new_total = total + len;
            let whole = i64::try_from(kept.len()).unwrap_or(i64::MAX) >= new_total;
            let blob = output.get("blob").cloned().unwrap_or(Json::Null);
            payload["tool"]["output"] = json!({
                "inline": kept, "total_len": new_total, "truncated": !whole, "blob": blob,
            });
        }
    }
}

/// The pieces held of the items that grow, for this process.
#[derive(Debug, Default)]
pub(crate) struct Growing {
    items: Mutex<HashMap<Key, Held>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Growing {
    /// Where the next piece of `field` of the item must begin, when anything of that field is held.
    pub(crate) fn expects(&self, key: &Key, field: DeltaField) -> Option<i64> {
        lock(&self.items)
            .get(key)?
            .parts
            .iter()
            .find(|part| part.field == field)
            .map(|part| part.base + part.len)
    }

    /// Begins holding `field` of `item`, which is `base` long in the item now.
    pub(crate) fn begin(&self, key: &Key, item: &ItemRow, field: DeltaField, base: i64) {
        let mut items = lock(&self.items);
        let held = items.entry(key.clone()).or_insert_with(|| Held {
            item: item.id,
            thread: item.thread_id,
            segment: key.0,
            turn: item.turn_id,
            stream: item.stream.clone(),
            parts: Vec::new(),
            since: None,
        });
        if !held.parts.iter().any(|part| part.field == field) {
            held.parts.push(Part {
                field,
                base,
                len: 0,
                data: String::new(),
            });
        }
    }

    /// Takes in a piece that begins where what is held of its field ends, and answers what it was a piece of. An
    /// item of which nothing is held is let go when a piece of it is not taken, so that it is read again.
    pub(crate) fn add(&self, key: &Key, delta: &ItemDelta) -> Option<Held> {
        let mut items = lock(&self.items);
        let held = items.get_mut(key)?;
        let Some(part) = held
            .parts
            .iter_mut()
            .find(|part| part.field == delta.field && part.base + part.len == delta.offset)
        else {
            if held.parts.iter().all(|part| part.len == 0) {
                items.remove(key);
            }
            return None;
        };
        part.add(&delta.data, delta.len);
        held.since.get_or_insert_with(Instant::now);
        Some(Held {
            item: held.item,
            thread: held.thread,
            segment: held.segment,
            turn: held.turn,
            stream: held.stream.clone(),
            parts: Vec::new(),
            since: held.since,
        })
    }

    /// What is held of the items of a thread.
    pub(crate) fn of_thread(&self, thread: Id) -> Vec<Held> {
        lock(&self.items)
            .values()
            .filter(|held| held.thread == thread && held.parts.iter().any(|part| part.len > 0))
            .cloned()
            .collect()
    }

    /// What is held whose oldest piece has waited `wait` or longer.
    pub(crate) fn due(&self, wait: Duration) -> Vec<(Key, Held)> {
        lock(&self.items)
            .iter()
            .filter(|(_, held)| held.since.is_some_and(|since| since.elapsed() >= wait))
            .map(|(key, held)| (key.clone(), held.clone()))
            .collect()
    }

    /// What `written` held is in its item now, which reaches `reached`: it is let go, and pieces that came since
    /// stay. An item of which nothing is left held is let go.
    pub(crate) fn written(&self, key: &Key, written: &Held, reached: &Reached) {
        let mut items = lock(&self.items);
        let Some(held) = items.get_mut(key).filter(|held| held.item == written.item) else {
            return;
        };
        for (field, at) in reached {
            if let Some(part) = held.parts.iter_mut().find(|part| part.field == *field) {
                part.reached(*at);
            }
        }
        if held.parts.iter().all(|part| part.len == 0) {
            items.remove(key);
        } else {
            held.since = Some(Instant::now());
        }
    }

    /// Lets the item go: it ended, which carries its whole text, or it is no longer what was held of it.
    pub(crate) fn forget(&self, key: &Key) {
        lock(&self.items).remove(key);
    }
}

impl Engine {
    /// Writes what is held of the items of `thread` into them, in the transaction of `unit`, whose caller holds the
    /// thread's row: what the session does next reads them whole. What is held stays until the writer of held text
    /// finds it written, since this transaction may yet be undone.
    pub(crate) async fn write_held_of(
        &self,
        unit: &mut Unit<'_>,
        thread: Id,
    ) -> Result<(), EngineError> {
        for held in self.inner.growing.of_thread(thread) {
            self.write_held(unit, &held).await?;
        }
        Ok(())
    }

    /// Writes to its item what `held` has that the item has not, stamping the item. The answer is how far each
    /// field of the item reaches then, or none where the item no longer grows or is not what was held of it.
    async fn write_held(
        &self,
        unit: &mut Unit<'_>,
        held: &Held,
    ) -> Result<Option<Reached>, EngineError> {
        let Some(item) = threads::item(&mut *unit.conn, held.item).await? else {
            return Ok(None);
        };
        if item.status != "in_progress" {
            return Ok(None);
        }
        let mut payload = item.payload.clone();
        let mut reached = Vec::with_capacity(held.parts.len());
        for part in &held.parts {
            let now = length_of(&payload, part.field);
            if now < part.base || now > part.base + part.len {
                return Ok(None);
            }
            let skip = now - part.base;
            if skip < part.len {
                append(&mut payload, part.field, &part.after(skip), part.len - skip);
            }
            reached.push((part.field, part.base + part.len));
        }
        if payload != item.payload {
            let seq = unit.stamp(held.thread).await?;
            let at = unit.now();
            threads::change_item(
                &mut *unit.conn,
                item.id,
                &ItemChange {
                    status: None,
                    payload: &payload,
                    search_text: None,
                    at,
                },
                seq,
            )
            .await?;
            if orch_db::queries::thread_history::selected_segment(
                unit.conn,
                held.thread,
                held.segment,
            )
            .await?
            {
                threads::touch(&mut *unit.conn, held.thread, at).await?;
            }
        }
        Ok(Some(reached))
    }

    /// Writes held text whose oldest piece has waited an interval, each item in a transaction of its own, until the
    /// engine stops, when all that is held is written.
    pub(crate) async fn write_held_text(&self) {
        let every = self.inner.config.stream_flush;
        if every.is_zero() {
            return;
        }
        let tick = (every / 4).max(Duration::from_millis(25));
        loop {
            let stopping = self.is_stopped();
            let wait = if stopping { Duration::ZERO } else { every };
            for (key, held) in self.inner.growing.due(wait) {
                match self.write_alone(&held).await {
                    Ok(Some(reached)) => self.inner.growing.written(&key, &held, &reached),
                    Ok(None) => self.inner.growing.forget(&key),
                    Err(error) => {
                        tracing::warn!(%error, item = %held.item, "the text an item gained could not be written; it is tried again");
                    }
                }
            }
            if stopping {
                return;
            }
            tokio::select! {
                () = tokio::time::sleep(tick) => {}
                () = self.inner.signal.notified() => {}
            }
        }
    }

    /// Writes what is held of one item in a transaction of its own, which holds its thread's row as a delivery does.
    async fn write_alone(&self, held: &Held) -> Result<Option<Reached>, EngineError> {
        let mut tx = self.inner.db.begin().await?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        if threads::lock(&mut tx, held.thread).await?.is_none() {
            return Ok(None);
        }
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        let reached = self.write_held(&mut unit, held).await?;
        unit.finish().await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(reached)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(field: DeltaField, base: i64) -> Part {
        Part {
            field,
            base,
            len: 0,
            data: String::new(),
        }
    }

    #[test]
    fn what_was_written_is_let_go_and_what_came_since_stays() {
        let mut text = part(DeltaField::Text, 3);
        text.add("héllo", 5);
        text.add(" world", 6);
        assert_eq!(text.after(2), "llo world", "counted in characters");
        text.reached(3 + 5);
        assert_eq!((text.base, text.len, text.data.as_str()), (8, 6, " world"));
        text.reached(3);
        assert_eq!(text.base, 8, "an older reach takes nothing back");
        text.reached(100);
        assert_eq!((text.base, text.len, text.data.as_str()), (14, 0, ""));
    }

    #[test]
    fn of_a_long_output_the_end_an_item_keeps_is_held() {
        let mut output = part(DeltaField::Output, 0);
        let long = "x".repeat(OUTPUT_INLINE_MAX_BYTES);
        output.add(&long, i64::try_from(long.len()).unwrap());
        output.add("tail", 4);
        assert_eq!(output.len, i64::try_from(long.len()).unwrap() + 4);
        assert_eq!(output.data.len(), OUTPUT_INLINE_MAX_BYTES);
        assert!(output.data.ends_with("tail"));
        assert_eq!(output.after(output.len - 4), "tail");
        output.reached(output.len - 2);
        assert_eq!((output.len, output.data.as_str()), (2, "il"));
    }

    #[test]
    fn appending_a_field_writes_what_a_piece_would() {
        let mut message = json!({ "text": "ab", "text_len": 2 });
        append(&mut message, DeltaField::Text, "çd", 2);
        assert_eq!(message, json!({ "text": "abçd", "text_len": 4 }));
        assert_eq!(length_of(&message, DeltaField::Text), 4);
        let mut call = json!({ "tool": { "kind": "command", "output": { "inline": "a", "total_len": 1, "truncated": false, "blob": null } } });
        append(&mut call, DeltaField::Output, "bc", 2);
        assert_eq!(call["tool"]["output"]["inline"], "abc");
        assert_eq!(length_of(&call, DeltaField::Output), 3);
    }

    #[test]
    fn a_piece_that_does_not_continue_what_is_held_is_not_taken() {
        let growing = Growing::default();
        let key = (orch_types::new_id(), "m".to_owned());
        lock(&growing.items).insert(
            key.clone(),
            Held {
                item: orch_types::new_id(),
                thread: orch_types::new_id(),
                segment: key.0,
                turn: None,
                stream: "main".to_owned(),
                parts: vec![part(DeltaField::Text, 4)],
                since: None,
            },
        );
        let piece = |offset: i64, data: &str| ItemDelta {
            native_ref: "m".to_owned(),
            field: DeltaField::Text,
            offset,
            len: i64::try_from(data.chars().count()).unwrap(),
            data: data.to_owned(),
        };
        assert!(growing.add(&key, &piece(4, "ef")).is_some());
        assert!(
            growing.add(&key, &piece(0, "abcd")).is_none(),
            "already in the item"
        );
        assert!(growing.add(&key, &piece(9, "x")).is_none(), "skips ahead");
        assert_eq!(growing.expects(&key, DeltaField::Text), Some(6));
        assert_eq!(growing.expects(&key, DeltaField::Summary), None);
        assert_eq!(growing.due(Duration::ZERO).len(), 1);
        let other = (key.0, "n".to_owned());
        let like = lock(&growing.items)[&key].clone();
        lock(&growing.items).insert(
            other.clone(),
            Held {
                parts: vec![part(DeltaField::Text, 0)],
                since: None,
                ..like
            },
        );
        assert!(growing.add(&other, &piece(3, "x")).is_none());
        assert!(
            !lock(&growing.items).contains_key(&other),
            "an item of which nothing is held is read again"
        );
    }
}
