//! A history read a page at a time, turn by turn, from its live tables or from a captured cut, oldest or newest first.
//!
//! A reader holds the page it is reading, the turn it has not finished and a short queue of the turn rows ahead, and
//! whoever asks for turns gets a few at a time. The neutral carry bounds eligible item bytes before a turn is complete;
//! the ordinary written-form readers still assemble a complete turn, which can span many pages. The turns use their index
//! (their numbers), the items of the main stream from the order the history keeps them (one index range for the live
//! tables, one for a cut), grouped by the turn they were made in: a history is read by position, a page at a time, and
//! the index of the items by turn (`items_turn_idx`) is for the counts of a part of a history and the written form of a
//! few turns, not for this. A turn that made no item still comes, with none. The lines of the history that belong to no
//! turn go with the turn that follows them, or with the one they lie inside, as every written form of a conversation
//! puts them.

use std::collections::VecDeque;

use orch_db::Conn;
use orch_db::queries::thread_continuations::reading;
use orch_db::queries::thread_history;
use orch_db::queries::thread_history_cuts as cuts;
use orch_db::queries::threads::{ItemRow, TurnRow};
use orch_types::{Id, Seq};

use crate::error::EngineError;

/// How many turn rows are read at a time.
const TURN_PAGE: i64 = 100;

/// Where a history is read from.
#[derive(Clone, Copy)]
pub(crate) enum Source<'a> {
    /// The selected history of a thread, as one view of it sees it.
    Live {
        /// The thread.
        thread: Id,
        /// The view.
        view: Id,
    },
    /// A history captured once, whose membership and bodies no longer change.
    Cut(&'a cuts::Cut),
}

impl Source<'_> {
    /// A page of the main stream after the position `after`, oldest first.
    async fn items_after(&self, conn: &mut Conn, after: Seq) -> Result<Vec<ItemRow>, EngineError> {
        Ok(match self {
            Self::Live { thread, view } => {
                reading::items(
                    conn,
                    *thread,
                    *view,
                    Some(after),
                    true,
                    Some("main"),
                    cuts::PAGE,
                )
                .await?
            }
            Self::Cut(cut) => {
                cuts::items_page(conn, cut.id, after, cut.captured_seq, Some("main")).await?
            }
        })
    }

    /// A page of the main stream before the position `before` (the end of the history with none), oldest first.
    pub(crate) async fn items_before(
        &self,
        conn: &mut Conn,
        before: Option<Seq>,
    ) -> Result<Vec<ItemRow>, EngineError> {
        Ok(match self {
            Self::Live { thread, view } => {
                reading::items(
                    conn,
                    *thread,
                    *view,
                    before,
                    false,
                    Some("main"),
                    cuts::PAGE,
                )
                .await?
            }
            Self::Cut(cut) => cuts::items_page_back(conn, cut.id, before).await?,
        })
    }

    /// A page of the turns numbered above `after` and up to `high`, oldest first.
    async fn turns_after(
        &self,
        conn: &mut Conn,
        after: i64,
        high: i64,
    ) -> Result<Vec<TurnRow>, EngineError> {
        Ok(match self {
            Self::Live { thread, view } => {
                thread_history::turns_after(conn, *thread, *view, after, high, TURN_PAGE).await?
            }
            Self::Cut(cut) => cuts::turns_after_page(conn, cut.id, after, high, TURN_PAGE).await?,
        })
    }

    /// A page of the turns numbered below `before` and from `low`, the latest ones, oldest first.
    async fn turns_before(
        &self,
        conn: &mut Conn,
        before: i64,
        low: i64,
    ) -> Result<Vec<TurnRow>, EngineError> {
        Ok(match self {
            Self::Live { thread, view } => {
                let before = i32::try_from(before).ok();
                let mut page = reading::turns(conn, *thread, *view, before, TURN_PAGE).await?;
                page.retain(|turn| i64::from(turn.ordinal) >= low);
                page
            }
            Self::Cut(cut) => cuts::turns_before_page(conn, cut.id, before, low, TURN_PAGE).await?,
        })
    }

    /// The position of the last main-stream item through the turn numbered `ordinal`: where a read of the turns after it
    /// begins, and a read of the turns up to it ends.
    pub(crate) async fn high_through(
        &self,
        conn: &mut Conn,
        ordinal: i64,
    ) -> Result<Option<Seq>, EngineError> {
        Ok(match self {
            Self::Live { thread, view } => {
                thread_history::turn_high(conn, *thread, *view, ordinal).await?
            }
            Self::Cut(cut) => cuts::turn_high(conn, cut.id, ordinal).await?,
        })
    }
}

/// One turn with the items of the main stream that were made in it, and the lines that go with it, oldest first.
pub(crate) struct Bundle {
    /// The turn.
    pub turn: TurnRow,
    /// Its items, in the order the history keeps them.
    pub items: Vec<ItemRow>,
}

/// What a reader has read so far: pages and rows, and the most it held at once. A caller (and a test) can see from it that
/// the cost of a read follows what it asked for and not the length of the history.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the size tests read what a reader read; the code only keeps it"
    )
)]
pub(crate) struct Reads {
    /// Pages of items read.
    pub item_pages: u32,
    /// Items read.
    pub items: u64,
    /// Pages of turn rows read.
    pub turn_pages: u32,
    /// Turn rows read.
    pub turns: u64,
    /// The most rows (turns and items) held at once.
    pub peak_held: usize,
}

/// The turn rows ahead of a reader, in the order it reads them.
struct Queue {
    pages: u32,
    read: u64,
    forward: bool,
    /// Forward: the number the next page begins after. Backward: the number the next page ends before.
    edge: i64,
    range: (i64, i64),
    rows: VecDeque<TurnRow>,
    exhausted: bool,
}

impl Queue {
    fn new(forward: bool, range: (i64, i64)) -> Self {
        Self {
            pages: 0,
            read: 0,
            forward,
            edge: if forward { range.0 - 1 } else { range.1 + 1 },
            range,
            rows: VecDeque::new(),
            exhausted: range.0 > range.1,
        }
    }

    /// Reads the next page of turn rows, if there is one.
    async fn refill(&mut self, source: &Source<'_>, conn: &mut Conn) -> Result<(), EngineError> {
        if self.exhausted {
            return Ok(());
        }
        let page = if self.forward {
            source.turns_after(conn, self.edge, self.range.1).await?
        } else {
            let mut page = source.turns_before(conn, self.edge, self.range.0).await?;
            page.reverse();
            page
        };
        self.pages += 1;
        self.read += page.len() as u64;
        self.exhausted = page.len() < usize::try_from(TURN_PAGE).unwrap_or(usize::MAX);
        if let Some(last) = page.last() {
            self.edge = i64::from(last.ordinal);
        }
        self.rows.extend(page);
        Ok(())
    }

    /// Where the turn is in the current metadata page. A distant turn stays on the unread item page while
    /// preceding empty turns are returned to the caller; finding it never retains their metadata together.
    fn locate(&self, id: Id) -> Option<usize> {
        self.rows.iter().position(|turn| turn.id == id)
    }
}

/// Reads turns oldest first.
pub(crate) struct Forward<'a> {
    source: Source<'a>,
    after: Seq,
    trailing: bool,
    reads: Reads,
    queue: Queue,
    current: Option<(TurnRow, Vec<ItemRow>)>,
    pending: Vec<ItemRow>,
    unread: VecDeque<ItemRow>,
    ready: VecDeque<Bundle>,
    ended: bool,
}

impl<'a> Forward<'a> {
    /// A reader of the turns numbered `low` to `high`, found where they begin. With `trailing`, the lines after the last item
    /// of the last turn go with that turn.
    pub(crate) async fn between(
        source: Source<'a>,
        conn: &mut Conn,
        range: (i64, i64),
        trailing: bool,
    ) -> Result<Self, EngineError> {
        let after = if range.0 <= 1 {
            0
        } else {
            source.high_through(conn, range.0 - 1).await?.unwrap_or(0)
        };
        Ok(Self {
            source,
            after,
            trailing,
            reads: Reads::default(),
            queue: Queue::new(true, range),
            current: None,
            pending: Vec::new(),
            unread: VecDeque::new(),
            ready: VecDeque::new(),
            ended: false,
        })
    }

    fn finish_current(&mut self) {
        if let Some((turn, items)) = self.current.take() {
            self.ready.push_back(Bundle { turn, items });
        }
    }

    /// The rows held beside the page being read.
    fn held(&self) -> usize {
        self.queue.rows.len()
            + self.pending.len()
            + self.unread.len()
            + self
                .current
                .as_ref()
                .map_or(0, |(_, items)| items.len() + 1)
            + self
                .ready
                .iter()
                .map(|bundle| bundle.items.len() + 1)
                .sum::<usize>()
    }

    /// What has been read, and the most held at once.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "the size tests read what a reader read; the code only keeps it"
        )
    )]
    pub(crate) fn reads(&self) -> Reads {
        Reads {
            turn_pages: self.queue.pages,
            turns: self.queue.read,
            ..self.reads
        }
    }

    /// The next turns, at most `most` of them; none when the range is read.
    pub(crate) async fn next(
        &mut self,
        conn: &mut Conn,
        most: usize,
    ) -> Result<Vec<Bundle>, EngineError> {
        while self.ready.len() < most && !self.ended {
            if self.unread.is_empty() {
                let page = self.source.items_after(conn, self.after).await?;
                self.reads.item_pages += 1;
                self.reads.items += page.len() as u64;
                self.reads.peak_held = self.reads.peak_held.max(page.len() + self.held());
                let Some(last) = page.last().map(|item| item.seq_created) else {
                    // The end of the history: the lines after the last item go with the last turn, where the form says so,
                    // and the turns that follow it made no item.
                    if let Some((_, items)) = self.current.as_mut() {
                        if self.trailing {
                            items.append(&mut self.pending);
                        } else {
                            self.pending.clear();
                        }
                    }
                    self.finish_current();
                    self.end_with_empty_turns(conn, most).await?;
                    break;
                };
                self.after = last;
                self.unread = page.into();
            }
            let Some(item) = self.unread.pop_front() else {
                continue;
            };
            let Some(turn) = item.turn_id else {
                if item.kind != "reasoning" {
                    self.pending.push(item);
                }
                continue;
            };
            if item.kind == "reasoning" {
                continue;
            }
            if self
                .current
                .as_ref()
                .is_some_and(|(current, _)| current.id == turn)
            {
                if let Some((_, items)) = self.current.as_mut() {
                    items.append(&mut self.pending);
                    items.push(item);
                }
                continue;
            }
            if self.queue.rows.is_empty() {
                self.queue.refill(&self.source, conn).await?;
                self.reads.peak_held = self.reads.peak_held.max(self.held());
            }
            // The item being placed is held beside the unread page until it joins a bundle.
            self.reads.peak_held = self.reads.peak_held.max(self.held() + 1);
            match self.queue.locate(turn) {
                Some(at) => {
                    self.finish_current();
                    // The turns between made no item.
                    for _ in 0..at {
                        if let Some(turn) = self.queue.rows.pop_front() {
                            self.ready.push_back(Bundle {
                                turn,
                                items: Vec::new(),
                            });
                        }
                    }
                    if let Some(row) = self.queue.rows.pop_front() {
                        let mut items = std::mem::take(&mut self.pending);
                        items.push(item);
                        self.current = Some((row, items));
                    }
                }
                None if self.queue.exhausted => {
                    // A turn after the range: what lies before its first line goes with it, and the read is over.
                    self.pending.clear();
                    self.unread.clear();
                    self.finish_current();
                    self.ended = true;
                    break;
                }
                None => {
                    self.finish_current();
                    while self.ready.len() < most {
                        let Some(turn) = self.queue.rows.pop_front() else {
                            break;
                        };
                        self.ready.push_back(Bundle {
                            turn,
                            items: Vec::new(),
                        });
                    }
                    self.unread.push_front(item);
                }
            }
        }
        self.reads.peak_held = self.reads.peak_held.max(self.held());
        if self.ended {
            self.end_with_empty_turns(conn, most).await?;
        }
        let take = most.min(self.ready.len());
        Ok(self.ready.drain(..take).collect())
    }

    /// The turns left in the range made no item: they are read as turns with none, as they are asked for.
    async fn end_with_empty_turns(
        &mut self,
        conn: &mut Conn,
        most: usize,
    ) -> Result<(), EngineError> {
        self.ended = true;
        while self.ready.len() < most {
            if self.queue.rows.is_empty() {
                self.queue.refill(&self.source, conn).await?;
                self.reads.peak_held = self.reads.peak_held.max(self.held());
            }
            let Some(turn) = self.queue.rows.pop_front() else {
                break;
            };
            self.ready.push_back(Bundle {
                turn,
                items: Vec::new(),
            });
        }
        Ok(())
    }
}

/// Reads turns newest first.
pub(crate) struct Backward<'a> {
    source: Source<'a>,
    before: Option<Seq>,
    trailing: bool,
    reads: Reads,
    queue: Queue,
    current: Option<(TurnRow, Vec<ItemRow>)>,
    pending: Vec<ItemRow>,
    unread: VecDeque<ItemRow>,
    ready: VecDeque<Bundle>,
    ended: bool,
    neutral_room: Option<usize>,
    neutral_overflow: bool,
}

impl<'a> Backward<'a> {
    /// A reader of the turns numbered `range.0` to `range.1`, found where they end. `last` is the number of the last turn there
    /// is. With `trailing`, the lines after the last item of the last turn go with that turn.
    pub(crate) async fn between(
        source: Source<'a>,
        conn: &mut Conn,
        range: (i64, i64),
        last: i64,
        trailing: bool,
    ) -> Result<Self, EngineError> {
        let before = if range.1 >= last {
            None
        } else {
            source
                .high_through(conn, range.1)
                .await?
                .map(|seq| seq.saturating_add(1))
        };
        Ok(Self {
            source,
            before,
            trailing,
            reads: Reads::default(),
            queue: Queue::new(false, range),
            current: None,
            pending: Vec::new(),
            unread: VecDeque::new(),
            ready: VecDeque::new(),
            ended: false,
            neutral_room: None,
            neutral_overflow: false,
        })
    }

    /// Limits eligible neutral items while assembling a turn, before an oversized turn can be held whole. Previously
    /// completed turns remain available; the unfinished turn and the older range are left out, as the carry requires.
    pub(crate) fn with_neutral_bound(mut self, bytes: usize) -> Self {
        self.neutral_room = Some(bytes);
        self
    }

    /// Whether a turn was left unfinished because its neutral items could not fit.
    pub(crate) fn neutral_overflowed(&self) -> bool {
        self.neutral_overflow
    }

    /// A bounded neutral reader retains only what neutral_lines would keep. Its allowance is the exact serialization,
    /// including each newline; metadata and non-model notices do not consume it or accumulate beside a large turn.
    fn admit_neutral(&mut self, item: &ItemRow, ordinal: i32) -> bool {
        let Some(room) = self.neutral_room else {
            return true;
        };
        if (item.turn_id.is_none() && !item.before_orch) || !crate::fork_history::is_seen(item) {
            return false;
        }
        let bytes = crate::fork_history::neutral_line(item, ordinal)
            .len()
            .saturating_add(1);
        if let Some(left) = room.checked_sub(bytes) {
            self.neutral_room = Some(left);
            return true;
        }
        self.neutral_overflow = true;
        self.current = None;
        self.pending.clear();
        self.unread.clear();
        self.ended = true;
        false
    }

    fn finish_current(&mut self) {
        if let Some((turn, mut items)) = self.current.take() {
            items.reverse();
            self.ready.push_back(Bundle { turn, items });
        }
    }

    /// The rows held beside the page being read.
    fn held(&self) -> usize {
        self.queue.rows.len()
            + self.pending.len()
            + self.unread.len()
            + self
                .current
                .as_ref()
                .map_or(0, |(_, items)| items.len() + 1)
            + self
                .ready
                .iter()
                .map(|bundle| bundle.items.len() + 1)
                .sum::<usize>()
    }

    /// What has been read, and the most held at once.
    pub(crate) fn reads(&self) -> Reads {
        Reads {
            turn_pages: self.queue.pages,
            turns: self.queue.read,
            ..self.reads
        }
    }

    /// The next turns, newest first, at most `most` of them; none when the range is read. Each turn's items are oldest first.
    pub(crate) async fn next(
        &mut self,
        conn: &mut Conn,
        most: usize,
    ) -> Result<Vec<Bundle>, EngineError> {
        while self.ready.len() < most && !self.ended {
            if self.unread.is_empty() {
                let page = self.source.items_before(conn, self.before).await?;
                self.reads.item_pages += 1;
                self.reads.items += page.len() as u64;
                self.reads.peak_held = self.reads.peak_held.max(page.len() + self.held());
                let Some(first) = page.first().map(|item| item.seq_created) else {
                    if let Some((_, items)) = self.current.as_mut() {
                        items.append(&mut self.pending);
                    }
                    self.finish_current();
                    self.end_with_empty_turns(conn, most).await?;
                    break;
                };
                self.before = Some(first);
                self.unread = page.into_iter().rev().collect();
            }
            let Some(item) = self.unread.pop_front() else {
                continue;
            };
            let Some(turn) = item.turn_id else {
                if item.kind != "reasoning"
                    && (self.neutral_room.is_none() || self.current.is_some() || self.trailing)
                {
                    if self.admit_neutral(&item, 0) {
                        self.pending.push(item);
                    }
                    if self.neutral_overflow {
                        break;
                    }
                }
                continue;
            };
            if item.kind == "reasoning" {
                continue;
            }
            if let Some(ordinal) = self
                .current
                .as_ref()
                .and_then(|(current, _)| (current.id == turn).then_some(current.ordinal))
            {
                if self.admit_neutral(&item, ordinal)
                    && let Some((_, items)) = self.current.as_mut()
                {
                    items.append(&mut self.pending);
                    items.push(item);
                }
                if self.neutral_overflow {
                    break;
                }
                continue;
            }
            if self.queue.rows.is_empty() {
                self.queue.refill(&self.source, conn).await?;
                self.reads.peak_held = self.reads.peak_held.max(self.held());
            }
            // The item being placed is held beside the unread page until it joins a bundle.
            self.reads.peak_held = self.reads.peak_held.max(self.held() + 1);
            match self.queue.locate(turn) {
                Some(at) => {
                    let starting = self.current.is_none();
                    if let Some((_, items)) = self.current.as_mut() {
                        // Lines between the last item of this turn and the first of the one after it go with the
                        // one after it.
                        items.append(&mut self.pending);
                    }
                    self.finish_current();
                    // The turns between made no item.
                    for _ in 0..at {
                        if let Some(turn) = self.queue.rows.pop_front() {
                            self.ready.push_back(Bundle {
                                turn,
                                items: Vec::new(),
                            });
                        }
                    }
                    if let Some(row) = self.queue.rows.pop_front() {
                        let ordinal = row.ordinal;
                        let mut items = Vec::new();
                        if starting && self.trailing {
                            items.append(&mut self.pending);
                        }
                        self.pending.clear();
                        self.current = Some((row, items));
                        if self.admit_neutral(&item, ordinal)
                            && let Some((_, items)) = self.current.as_mut()
                        {
                            items.push(item);
                        }
                        if self.neutral_overflow {
                            break;
                        }
                    }
                }
                None if self.queue.exhausted => {
                    // Before the range: what lies after this line goes with the earliest turn read, and the read
                    // is over.
                    if let Some((_, items)) = self.current.as_mut() {
                        items.append(&mut self.pending);
                    }
                    self.pending.clear();
                    self.unread.clear();
                    self.finish_current();
                    self.ended = true;
                    break;
                }
                None => {
                    if let Some((_, items)) = self.current.as_mut() {
                        items.append(&mut self.pending);
                    }
                    self.finish_current();
                    while self.ready.len() < most {
                        let Some(turn) = self.queue.rows.pop_front() else {
                            break;
                        };
                        self.ready.push_back(Bundle {
                            turn,
                            items: Vec::new(),
                        });
                    }
                    self.unread.push_front(item);
                }
            }
        }
        self.reads.peak_held = self.reads.peak_held.max(self.held());
        if self.ended && !self.neutral_overflow {
            self.end_with_empty_turns(conn, most).await?;
        }
        let take = most.min(self.ready.len());
        Ok(self.ready.drain(..take).collect())
    }

    /// The turns left in the range made no item: they are read as turns with none, as they are asked for.
    async fn end_with_empty_turns(
        &mut self,
        conn: &mut Conn,
        most: usize,
    ) -> Result<(), EngineError> {
        self.ended = true;
        while self.ready.len() < most {
            if self.queue.rows.is_empty() {
                self.queue.refill(&self.source, conn).await?;
                self.reads.peak_held = self.reads.peak_held.max(self.held());
            }
            let Some(turn) = self.queue.rows.pop_front() else {
                break;
            };
            self.ready.push_back(Bundle {
                turn,
                items: Vec::new(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
