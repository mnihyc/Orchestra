//! Sequence and reload contracts.
//!
//! A durable feed numbers its rows. A client loads the state of a feed
//! over HTTP, says how far it has come, and is sent every row stamped
//! after that, in order and none skipped. The arithmetic of that promise
//! is here, once for each side, as plain values that are told what
//! happened and say what to do: [`FeedCursor`] is what a client keeps for
//! each feed it follows, [`SendCursor`] what a server keeps for each
//! subscription, and [`FeedHead::answer`] is how a server answers a
//! subscription. Nothing here reads a clock, a socket or a database.
//!
//! The stamps of the rows a subscriber sees are not consecutive: rows
//! they may not see, and rows of the other tables that share the feed's
//! numbers, take numbers too. So a frame says where its range of stamps
//! begins, and a client compares that with where it stands. That is the
//! difference from the arithmetic of a stream whose entries are numbered
//! one after the other.

use crate::feeds::FeedKind;
use orch_types::Seq;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How a subscription begins, as the server answers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubscribeMode {
    /// Every row stamped after the starting point follows, in order.
    Delta,
    /// The starting point is no use: the client discards what it holds
    /// for the feed, loads it again and subscribes again. The server does
    /// not keep a subscription it answered like this, and sends no row
    /// on it.
    Reload,
    /// The feed has no sequence: events follow while it is subscribed,
    /// and nothing that happened before.
    Ephemeral,
}

// ------------------------------------------------------------------ the client

/// What a client does with a `sync` frame, or with a `resync` frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameAction {
    /// Apply its rows, each only where it is newer than the stored copy
    /// ([`supersedes`]). The cursor has moved to the frame's `seq`.
    Apply,
    /// Everything in it was applied already: ignore it. The cursor stays.
    Drop,
    /// Rows were missed: read the feed's HTTP request with `since` set to
    /// [`FeedCursor::last_seq`], apply the answer, and tell the cursor
    /// with [`FeedCursor::on_repaired`].
    Resync,
}

/// What a client does after the server answered a subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubscribeOutcome {
    /// Follow the frames that come: the server will send everything
    /// after the starting point it was given.
    Follow,
    /// The stream begins after what the client holds, so rows lie
    /// between: read them over HTTP as for a [`FrameAction::Resync`].
    Resync,
    /// Discard what is held for the feed, load it again, subscribe again.
    Reload,
}

/// What a client does after it applied an HTTP answer that repaired a feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepairOutcome {
    /// The store is up to date: go on following the frames.
    Continue,
    /// The answer could not repair it: discard what is held, load again,
    /// subscribe again.
    Reload,
}

/// Whether a row that arrived replaces the copy a client stores: it does
/// when the client has none, and when the stored copy is older. A copy
/// with an equal or higher stamp is never replaced, so that a repair and
/// a frame that carry the same row, in either order, leave one result.
///
/// The stamp is the row's own `seq_updated`; for a tree node it is
/// `tree_seq`, and for a notification `inbox_seq`.
#[must_use]
pub const fn supersedes(stored: Option<Seq>, incoming: Seq) -> bool {
    match stored {
        None => true,
        Some(stored) => incoming > stored,
    }
}

/// How far a client has applied one durable feed.
///
/// A client makes one when it has loaded the feed over HTTP (the answer
/// says how far it is), subscribes with [`FeedCursor::last_seq`] as
/// `since`, and tells the cursor of every frame and answer that follows.
/// After a reconnect it subscribes again with the same number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FeedCursor {
    last_seq: Seq,
}

impl FeedCursor {
    /// The cursor of a feed that was loaded as of `seq`.
    #[must_use]
    pub const fn loaded(seq: Seq) -> Self {
        Self { last_seq: seq }
    }

    /// The highest sequence the client has applied: what it sends as
    /// `since` when it subscribes, and as `since` when it repairs.
    #[must_use]
    pub const fn last_seq(&self) -> Seq {
        self.last_seq
    }

    /// A `sync` frame carrying the rows stamped in `(prev, seq]`.
    ///
    /// A frame that ends at or before where the client stands is
    /// [`Drop`](FrameAction::Drop)ped, and must not move the cursor back.
    /// One that begins at or before it, and ends after, is
    /// [`Apply`](FrameAction::Apply)ed: every row it brings that the
    /// client lacks is in it, and the rows it has the client already has.
    /// One that begins after it left a hole, which is
    /// [`Resync`](FrameAction::Resync)ed.
    ///
    /// The case of a frame that begins before the cursor but ends after
    /// it is the one a repair makes: the answer of an HTTP request is
    /// read later than the frames that were in flight, and takes the
    /// client past some of them. Asking again for it would be an
    /// unnecessary request, not a wrong one.
    pub fn on_sync(&mut self, prev: Seq, seq: Seq) -> FrameAction {
        if seq <= self.last_seq {
            FrameAction::Drop
        } else if prev <= self.last_seq {
            self.last_seq = seq;
            FrameAction::Apply
        } else {
            FrameAction::Resync
        }
    }

    /// A `resync` frame: the server dropped what it could not send, and
    /// the client reads what it lacks over HTTP.
    #[must_use]
    pub const fn on_resync(&self) -> FrameAction {
        FrameAction::Resync
    }

    /// The server answered the subscription with `mode` and `seq`.
    ///
    /// For [`Delta`](SubscribeMode::Delta), `seq` is where the stream
    /// begins. It is what the client asked for unless the client asked
    /// for nothing, and a stream that begins after what the client holds
    /// leaves rows between, which are read over HTTP.
    pub fn on_subscribed(&mut self, mode: SubscribeMode, seq: Option<Seq>) -> SubscribeOutcome {
        match (mode, seq) {
            (SubscribeMode::Reload, _) => SubscribeOutcome::Reload,
            (SubscribeMode::Delta, Some(start)) if start > self.last_seq => {
                SubscribeOutcome::Resync
            }
            (SubscribeMode::Delta | SubscribeMode::Ephemeral, _) => SubscribeOutcome::Follow,
        }
    }

    /// The client applied the answer of the feed's HTTP request, read
    /// with `since` set to [`FeedCursor::last_seq`]: `seq` is the answer's
    /// sequence and `complete` its member of that name.
    ///
    /// An answer that is not complete says that more rows changed than a
    /// request from a sequence holds, so the client loads again from
    /// nothing. So does an answer that is behind what the client already
    /// holds: the server was restored from a copy that is older than the
    /// client's, and nothing the client holds can be trusted.
    pub fn on_repaired(&mut self, seq: Seq, complete: bool) -> RepairOutcome {
        if !complete || seq < self.last_seq {
            return RepairOutcome::Reload;
        }
        self.last_seq = seq;
        RepairOutcome::Continue
    }
}

// ------------------------------------------------------------------ the server

/// Why a server tells a client to load a feed again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReloadCause {
    /// The client's `since` is greater than the feed's sequence: the
    /// database was restored from an older copy.
    AheadOfFeed,
    /// More rows changed than the feed's HTTP request answers from a
    /// sequence, so the client would be sent a flood to stay as it is.
    TooManyRows,
    /// Rows that were removed after `since` have been purged for good, so
    /// that the client cannot be told of their removal. A `since` before
    /// the first sequence of any feed is treated the same way.
    Purged,
}

/// What a server answers to a subscription: the two members of
/// `subscribed` that it decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubscribeAnswer {
    /// The `seq` of the frame.
    pub seq: Option<Seq>,
    /// The `mode` of the frame.
    pub mode: SubscribeMode,
}

/// What a server knows of a durable feed when somebody subscribes to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedHead {
    /// The feed's sequence now.
    pub seq: Seq,
    /// The sequence up to which removed rows have been purged for good:
    /// 0 for a feed that purges nothing. Only a document's feed does.
    pub purged_seq: Seq,
    /// The most rows that the feed's HTTP request answers from a
    /// sequence; with more changed rows it answers that it is not
    /// complete.
    pub incremental_limit: i64,
}

impl FeedHead {
    /// Why a client that has the feed as of `since` cannot be sent what
    /// came after, given that `rows_after_since` rows are stamped above it;
    /// `None` when it can.
    #[must_use]
    pub fn reload_cause(&self, since: Seq, rows_after_since: i64) -> Option<ReloadCause> {
        if since > self.seq {
            Some(ReloadCause::AheadOfFeed)
        } else if since < self.purged_seq {
            Some(ReloadCause::Purged)
        } else if rows_after_since > self.incremental_limit {
            Some(ReloadCause::TooManyRows)
        } else {
            None
        }
    }

    /// The answer to `sub { since }` for a feed of `kind`.
    ///
    /// A feed without a sequence answers [`Ephemeral`](SubscribeMode::Ephemeral)
    /// with no `seq`. A durable feed whose client gave no `since` begins
    /// at the feed's sequence now: the client wants what happens from here.
    /// With a `since` it begins there, unless the client has to load again,
    /// in which case the answer is [`Reload`](SubscribeMode::Reload) and
    /// carries the feed's sequence now.
    #[must_use]
    pub fn answer(
        &self,
        kind: FeedKind,
        since: Option<Seq>,
        rows_after_since: i64,
    ) -> SubscribeAnswer {
        match (kind, since) {
            (FeedKind::Ephemeral, _) => SubscribeAnswer {
                seq: None,
                mode: SubscribeMode::Ephemeral,
            },
            (FeedKind::Durable, None) => SubscribeAnswer {
                seq: Some(self.seq),
                mode: SubscribeMode::Delta,
            },
            (FeedKind::Durable, Some(since)) => match self.reload_cause(since, rows_after_since) {
                Some(_) => SubscribeAnswer {
                    seq: Some(self.seq),
                    mode: SubscribeMode::Reload,
                },
                None => SubscribeAnswer {
                    seq: Some(since),
                    mode: SubscribeMode::Delta,
                },
            },
        }
    }
}

/// How far a server has read and sent one subscription.
///
/// Two numbers are kept. What was *sent* is the `seq` of the last frame,
/// or where the stream began: the `prev` of the next frame, which the
/// client compares with where it stands. What was *read* is the highest
/// stamp the server has looked at for this subscriber, whether it was
/// theirs to see or not: rows that a subscriber may not see, as in a tree
/// that is cut down for each person, are not read again at every sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SendCursor {
    sent: Seq,
    read: Seq,
}

impl SendCursor {
    /// The cursor of a subscription whose stream begins after `seq`.
    #[must_use]
    pub const fn starting_at(seq: Seq) -> Self {
        Self {
            sent: seq,
            read: seq,
        }
    }

    /// The `seq` of the last frame sent, or where the stream began.
    #[must_use]
    pub const fn sent(&self) -> Seq {
        self.sent
    }

    /// The stamps `(after, upto]` that the subscriber is still owed once
    /// the feed announced `notified`; `None` when it is owed none. The
    /// rows with those stamps are read once and sent as one frame.
    #[must_use]
    pub const fn owed(&self, notified: Seq) -> Option<(Seq, Seq)> {
        if notified > self.read {
            Some((self.read, notified))
        } else {
            None
        }
    }

    /// Whether the periodic sweep has something to repair: the feed has
    /// gone beyond what was read for this subscriber, and the
    /// announcement of it was lost.
    #[must_use]
    pub const fn behind(&self, feed_seq: Seq) -> bool {
        feed_seq > self.read
    }

    /// Records that the rows stamped up to `upto` were read, and that
    /// `highest` is the highest stamp among those the subscriber may see
    /// (`None` when they may see none). Returns the `(prev, seq)` of the
    /// frame to send: `None` when there is nothing to send, in which case
    /// no frame is sent and the next one still has the last frame's
    /// `seq` as its `prev`.
    pub fn read(&mut self, upto: Seq, highest: Option<Seq>) -> Option<(Seq, Seq)> {
        self.read = self.read.max(upto);
        let highest = highest.filter(|highest| *highest > self.sent)?;
        self.read = self.read.max(highest);
        let prev = self.sent;
        self.sent = highest;
        Some((prev, highest))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn a_frame_that_continues_where_the_client_stands_is_applied() {
        let mut cursor = FeedCursor::loaded(412);
        assert_eq!(cursor.on_sync(412, 414), FrameAction::Apply);
        assert_eq!(cursor.last_seq(), 414);
        assert_eq!(cursor.on_sync(414, 415), FrameAction::Apply);
        assert_eq!(cursor.last_seq(), 415);
    }

    #[test]
    fn a_frame_that_leaves_a_hole_is_a_resync_and_the_cursor_stays() {
        let mut cursor = FeedCursor::loaded(412);
        assert_eq!(cursor.on_sync(414, 415), FrameAction::Resync);
        assert_eq!(cursor.last_seq(), 412);
    }

    #[test]
    fn a_frame_the_client_has_already_is_dropped_and_never_moves_the_cursor_back() {
        let mut cursor = FeedCursor::loaded(420);
        assert_eq!(cursor.on_sync(414, 415), FrameAction::Drop);
        assert_eq!(cursor.on_sync(419, 420), FrameAction::Drop);
        assert_eq!(cursor.last_seq(), 420);
    }

    #[test]
    fn a_frame_that_reaches_beyond_a_repair_is_applied_without_asking_again() {
        // A repair took the client to 425; a frame in flight covers (420, 430].
        let mut cursor = FeedCursor::loaded(425);
        assert_eq!(cursor.on_sync(420, 430), FrameAction::Apply);
        assert_eq!(cursor.last_seq(), 430);
    }

    #[test]
    fn a_resync_frame_always_sends_the_client_to_read_what_it_lacks() {
        assert_eq!(FeedCursor::loaded(7).on_resync(), FrameAction::Resync);
    }

    #[test]
    fn a_repair_takes_the_client_to_the_answers_sequence_and_the_frame_that_asked_for_it_is_judged_again()
     {
        let mut cursor = FeedCursor::loaded(412);
        // The frame (414, 416] arrives while the client stands at 412.
        assert_eq!(cursor.on_sync(414, 416), FrameAction::Resync);
        // The answer was read a moment later, when the feed stood at 418.
        assert_eq!(cursor.on_repaired(418, true), RepairOutcome::Continue);
        assert_eq!(cursor.last_seq(), 418);
        assert_eq!(cursor.on_sync(414, 416), FrameAction::Drop);
        assert_eq!(cursor.on_sync(416, 418), FrameAction::Drop);
        assert_eq!(cursor.on_sync(418, 419), FrameAction::Apply);
    }

    #[test]
    fn an_answer_that_is_not_complete_or_is_behind_the_client_means_loading_again() {
        let mut cursor = FeedCursor::loaded(412);
        assert_eq!(cursor.on_repaired(9_000, false), RepairOutcome::Reload);
        assert_eq!(cursor.last_seq(), 412);
        assert_eq!(cursor.on_repaired(300, true), RepairOutcome::Reload);
        assert_eq!(cursor.last_seq(), 412);
        assert_eq!(cursor.on_repaired(412, true), RepairOutcome::Continue);
    }

    #[test]
    fn what_a_client_does_with_the_servers_answer_to_a_subscription() {
        let mut cursor = FeedCursor::loaded(412);
        assert_eq!(
            cursor.on_subscribed(SubscribeMode::Delta, Some(412)),
            SubscribeOutcome::Follow
        );
        assert_eq!(
            cursor.on_subscribed(SubscribeMode::Reload, Some(900)),
            SubscribeOutcome::Reload
        );
        assert_eq!(
            cursor.on_subscribed(SubscribeMode::Ephemeral, None),
            SubscribeOutcome::Follow
        );
        // A stream that begins after what the client holds leaves rows between.
        assert_eq!(
            cursor.on_subscribed(SubscribeMode::Delta, Some(430)),
            SubscribeOutcome::Resync
        );
        // One that begins before it only repeats rows, which are not applied twice.
        assert_eq!(
            cursor.on_subscribed(SubscribeMode::Delta, Some(400)),
            SubscribeOutcome::Follow
        );
        assert_eq!(cursor.last_seq(), 412);
    }

    #[test]
    fn a_row_replaces_the_stored_copy_only_when_it_is_new_or_newer() {
        assert!(supersedes(None, 1));
        assert!(supersedes(Some(5), 6));
        assert!(!supersedes(Some(5), 5));
        assert!(!supersedes(Some(5), 4));
    }

    fn head() -> FeedHead {
        FeedHead {
            seq: 500,
            purged_seq: 0,
            incremental_limit: 2_000,
        }
    }

    #[test]
    fn a_subscription_from_a_sequence_the_feed_can_continue_is_a_delta_from_there() {
        let answer = head().answer(FeedKind::Durable, Some(412), 88);
        assert_eq!(
            answer,
            SubscribeAnswer {
                seq: Some(412),
                mode: SubscribeMode::Delta
            }
        );
        // Exactly at the head, and exactly at the limit, are still deltas.
        assert_eq!(
            head().answer(FeedKind::Durable, Some(500), 0).mode,
            SubscribeMode::Delta
        );
        assert_eq!(
            head().answer(FeedKind::Durable, Some(1), 2_000).mode,
            SubscribeMode::Delta
        );
    }

    #[test]
    fn a_subscription_without_a_starting_point_begins_at_the_feed_now() {
        let answer = head().answer(FeedKind::Durable, None, 0);
        assert_eq!(
            answer,
            SubscribeAnswer {
                seq: Some(500),
                mode: SubscribeMode::Delta
            }
        );
    }

    #[test]
    fn a_feed_without_a_sequence_says_so() {
        let answer = head().answer(FeedKind::Ephemeral, Some(7), 0);
        assert_eq!(
            answer,
            SubscribeAnswer {
                seq: None,
                mode: SubscribeMode::Ephemeral
            }
        );
    }

    #[test]
    fn a_client_that_cannot_be_continued_is_told_to_load_the_feed_again() {
        let restored = head().reload_cause(501, 0);
        assert_eq!(restored, Some(ReloadCause::AheadOfFeed));
        let flood = head().reload_cause(10, 2_001);
        assert_eq!(flood, Some(ReloadCause::TooManyRows));
        let answer = head().answer(FeedKind::Durable, Some(501), 0);
        assert_eq!(
            answer,
            SubscribeAnswer {
                seq: Some(500),
                mode: SubscribeMode::Reload
            }
        );
    }

    #[test]
    fn a_document_reader_from_below_what_was_purged_loads_the_document_again() {
        let document = FeedHead {
            seq: 900,
            purged_seq: 300,
            incremental_limit: 2_000,
        };
        assert_eq!(document.reload_cause(299, 10), Some(ReloadCause::Purged));
        // At the purge mark itself nothing is missing: removed blocks above it are all still there.
        assert_eq!(document.reload_cause(300, 10), None);
        assert_eq!(
            document.answer(FeedKind::Durable, Some(0), 10).mode,
            SubscribeMode::Reload
        );
        // Rows are only ever purged from a feed that says so.
        assert_eq!(head().reload_cause(0, 10), None);
        // A starting point before the first sequence is no starting point.
        assert_eq!(
            head().answer(FeedKind::Durable, Some(-1), 0).mode,
            SubscribeMode::Reload
        );
    }

    #[test]
    fn a_server_sends_each_subscriber_what_it_is_owed_and_chains_the_frames() {
        let mut cursor = SendCursor::starting_at(412);
        assert_eq!(cursor.owed(412), None);
        assert_eq!(cursor.owed(414), Some((412, 414)));
        assert_eq!(cursor.read(414, Some(414)), Some((412, 414)));
        assert_eq!(cursor.owed(414), None);
        assert_eq!(cursor.read(415, Some(415)), Some((414, 415)));
        assert_eq!(cursor.sent(), 415);
    }

    #[test]
    fn rows_a_subscriber_may_not_see_send_no_frame_and_are_not_read_again() {
        let mut cursor = SendCursor::starting_at(100);
        // The rows stamped 101..=105 were all hidden from this subscriber.
        assert_eq!(cursor.owed(105), Some((100, 105)));
        assert_eq!(cursor.read(105, None), None);
        assert_eq!(cursor.sent(), 100);
        assert_eq!(cursor.owed(105), None);
        assert!(!cursor.behind(105));
        // The next frame still chains to the last the client heard.
        assert_eq!(cursor.read(110, Some(108)), Some((100, 108)));
        assert_eq!(cursor.owed(110), None);
    }

    #[test]
    fn rows_read_beyond_the_last_visible_one_are_not_read_again() {
        let mut cursor = SendCursor::starting_at(0);
        assert_eq!(cursor.read(20, Some(12)), Some((0, 12)));
        assert_eq!(cursor.sent(), 12);
        assert_eq!(cursor.owed(20), None);
        assert_eq!(cursor.owed(21), Some((20, 21)));
    }

    #[test]
    fn a_row_that_is_not_beyond_what_was_sent_is_not_a_frame() {
        let mut cursor = SendCursor::starting_at(50);
        assert_eq!(cursor.read(60, Some(50)), None);
        assert_eq!(cursor.read(60, Some(40)), None);
        assert_eq!(cursor.sent(), 50);
    }

    #[test]
    fn the_sweep_finds_a_subscriber_whose_announcement_was_lost() {
        let cursor = SendCursor::starting_at(100);
        assert!(!cursor.behind(100));
        assert!(cursor.behind(101));
    }

    // -------------------------------------------------- a feed, a link that loses frames, a client

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

    type Row = (u32, Seq, u64);

    /// The rows of a feed as a database keeps them: only the latest version of each.
    #[derive(Default)]
    struct Database {
        seq: Seq,
        rows: BTreeMap<u32, (Seq, u64)>,
    }

    impl Database {
        fn write(&mut self, row: u32, value: u64) {
            self.seq += 1;
            self.rows.insert(row, (self.seq, value));
        }

        /// Stamps a number that no subscriber sees, as another table of the feed would.
        fn skip(&mut self) {
            self.seq += 1;
        }

        fn after(&self, after: Seq, upto: Seq) -> Vec<Row> {
            self.rows
                .iter()
                .filter(|(_, (stamp, _))| *stamp > after && *stamp <= upto)
                .map(|(row, (stamp, value))| (*row, *stamp, *value))
                .collect()
        }
    }

    /// What a client stores: rows by id, replaced only by newer ones.
    #[derive(Default)]
    struct Store {
        rows: BTreeMap<u32, (Seq, u64)>,
    }

    impl Store {
        fn apply(&mut self, rows: &[Row]) {
            for (row, stamp, value) in rows {
                if supersedes(self.rows.get(row).map(|(held, _)| *held), *stamp) {
                    self.rows.insert(*row, (*stamp, *value));
                }
            }
        }
    }

    struct Frame {
        prev: Seq,
        seq: Seq,
        rows: Vec<Row>,
    }

    /// What the server does when the feed has moved: reads what the subscriber is owed, once, and frames it.
    fn send(database: &Database, sender: &mut SendCursor) -> Option<Frame> {
        let (after, upto) = sender.owed(database.seq)?;
        let rows = database.after(after, upto);
        let highest = rows.iter().map(|(_, stamp, _)| *stamp).max();
        let (prev, seq) = sender.read(upto, highest)?;
        Some(Frame { prev, seq, rows })
    }

    /// What the client does with a frame that reached it, repairing from the database when told to.
    fn deliver(frame: &Frame, client: &mut FeedCursor, store: &mut Store, database: &Database) {
        match client.on_sync(frame.prev, frame.seq) {
            FrameAction::Apply => store.apply(&frame.rows),
            FrameAction::Drop => {}
            FrameAction::Resync => {
                store.apply(&database.after(client.last_seq(), database.seq));
                assert_eq!(
                    client.on_repaired(database.seq, true),
                    RepairOutcome::Continue
                );
            }
        }
    }

    #[test]
    fn whatever_frames_a_link_loses_or_repeats_the_client_ends_with_the_servers_rows() {
        for game in 0..300_u64 {
            let mut dice = Dice(game);
            let mut database = Database::default();
            let mut sender = SendCursor::starting_at(0);
            let mut client = FeedCursor::loaded(0);
            let mut store = Store::default();
            let mut link: Vec<Frame> = Vec::new();

            for _ in 0..60 {
                // Writers change rows, and some of what they stamp nobody sees.
                for _ in 0..dice.below(4) {
                    if dice.below(5) == 0 {
                        database.skip();
                    } else {
                        let row = u32::try_from(dice.below(6)).unwrap();
                        database.write(row, dice.next());
                    }
                }
                // The server learns of it now and then, and sends what is owed.
                if dice.below(3) != 0 {
                    link.extend(send(&database, &mut sender));
                }
                // The link delivers some of what it holds, and loses and repeats frames.
                while !link.is_empty() && dice.below(2) == 0 {
                    let frame = link.remove(0);
                    match dice.below(10) {
                        0 => {}
                        1 => {
                            deliver(&frame, &mut client, &mut store, &database);
                            deliver(&frame, &mut client, &mut store, &database);
                        }
                        _ => deliver(&frame, &mut client, &mut store, &database),
                    }
                }
            }
            // What is still in flight arrives. A frame that was lost is found out by the next one, so
            // one more row is written, and its frame arrives.
            for frame in link.drain(..) {
                deliver(&frame, &mut client, &mut store, &database);
            }
            database.write(0, dice.next());
            let last = send(&database, &mut sender).expect("a row was just written");
            deliver(&last, &mut client, &mut store, &database);

            assert_eq!(store.rows, database.rows, "game {game}");
            assert_eq!(client.last_seq(), database.seq, "game {game}");
        }
    }
}
