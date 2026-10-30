//! Sequenced event delivery, replay and acknowledgement.
//!
//! What a machine tells the system travels in streams. The machine side is
//! [`StreamSender`]: it spools every event before it is sent, numbers events
//! as they go on the wire, keeps at most a window of them unacknowledged,
//! sends again from where the system says it stands after a break, a gap or a
//! long silence, and says with a reset when it can no longer send something
//! it was asked to. The system side is [`StreamReceiver`]: it hands the
//! caller each frame's new events to apply in one transaction, and only when
//! the caller says that it committed does it answer with an
//! acknowledgement. An event that has been taken in is never handed over
//! twice, and one that arrives out of its place is not handed over at all.
//!
//! Both are plain state machines: they take what happened and say which
//! frames to send. The arithmetic itself is the contract's ([`Outbox`] and
//! [`Inbox`]); this module adds the storage and the batching around it.

use crate::spool::{Numbered, Recovery, Relief, Spool, SpoolError, SpoolEvent, SpoolOptions};
use orch_link_contract::frames::Frame;
use orch_link_contract::session::Role;
use orch_link_contract::streams::{
    Ack, Answer, Eph, Ev, Evb, EvbEntry, Inbox, Nak, Outbox, Reset, ResetReason, Resume, Stream,
    is_diagnostic,
};
use orch_types::{Json, Seq, Time};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::Path;
use std::time::Duration;
use tokio::time::Instant;

/// How many diagnostics are let into the stream at a time, when nothing else
/// waits: a diagnostic must not hold up the event that comes after it, so
/// they go a few frames at a time and the question whether anything else
/// waits is asked again between.
pub const DIAGNOSTIC_BATCH: usize = 64;
/// How many numbered events are read from the spool and framed at a time.
const READ_BATCH: usize = 256;

/// Why an event could not be published.
#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    /// This machine has not switched on the role that reports on that
    /// stream.
    #[error("the role {0:?} reports on {1}, and this machine does not have it")]
    RoleMissing(Role, String),
    /// Events of that kind do not travel on that stream.
    #[error("events of kind {kind} do not travel on {stream}")]
    WrongStream {
        /// The kind.
        kind: String,
        /// The stream.
        stream: String,
    },
    /// One event is larger than a frame may be.
    #[error("an event of {bytes} bytes is larger than a frame of {max}")]
    TooLarge {
        /// Its size as it would be written.
        bytes: usize,
        /// The most a frame may have.
        max: usize,
    },
    /// The spool failed.
    #[error("{0}")]
    Spool(#[from] SpoolError),
}

/// How a stream stands, for whoever feeds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pressure {
    /// Events sent and not yet acknowledged.
    pub unacknowledged_events: i64,
    /// Their size in bytes.
    pub unacknowledged_bytes: i64,
    /// Events spooled and not yet sent.
    pub waiting_events: usize,
    /// Whether the window is full: whoever produces events should stop
    /// reading from its source until this clears.
    pub blocked: bool,
}

#[derive(Debug)]
struct StreamState {
    outbox: Outbox,
    /// The number of the next event to put on the wire. Behind the last
    /// number given, it is a resend in progress.
    next_to_wire: Seq,
    unacknowledged_bytes: i64,
    /// A reset the system has to be told of, and has not been yet.
    reset_owed: Option<(Seq, ResetReason)>,
    /// Whether the reset owed has been sent on this session: a reset that
    /// went out once is not repeated, because the system may have moved on
    /// and would be set back by it.
    reset_sent: bool,
    /// When something last happened to this stream that moves it on: an
    /// acknowledgement, a request to send again, or a frame being sent.
    last_progress: Instant,
}

/// The machine's side of its streams.
#[derive(Debug)]
pub struct StreamSender {
    spool: Spool,
    streams: HashMap<String, StreamState>,
    roles: Vec<Role>,
    connected: bool,
    max_frame_bytes: usize,
}

fn entry_of(numbered: &Numbered) -> EvbEntry {
    EvbEntry {
        seq: numbered.seq,
        at: numbered.event.at,
        kind: numbered.event.kind.clone(),
        body: numbered.event.body.clone(),
    }
}

impl StreamSender {
    /// Opens the spool in `dir` and picks up where an earlier run stopped.
    /// `roles` are those the machine has switched on.
    pub fn open(
        dir: &Path,
        options: SpoolOptions,
        roles: Vec<Role>,
        now: Instant,
    ) -> Result<(Self, Recovery), SpoolError> {
        let (spool, recovery) = Spool::open(dir, options)?;
        Ok(Self::opened(spool, recovery, roles, now))
    }

    /// Opens machine data with allocators outside the spool bytes, which may be lost as a whole.
    pub fn open_for_machine(
        data_dir: &Path,
        options: SpoolOptions,
        roles: Vec<Role>,
        now: Instant,
    ) -> Result<(Self, Recovery), SpoolError> {
        let (spool, recovery) = Spool::open_for_machine(data_dir, options)?;
        Ok(Self::opened(spool, recovery, roles, now))
    }

    fn opened(
        spool: Spool,
        recovery: Recovery,
        roles: Vec<Role>,
        now: Instant,
    ) -> (Self, Recovery) {
        let mut sender = Self {
            spool,
            streams: HashMap::new(),
            roles,
            connected: false,
            max_frame_bytes: orch_link_contract::version::MAX_FRAME_BYTES as usize,
        };
        for stream in sender.spool.streams() {
            let state = sender.state_from_spool(&stream, now);
            sender.streams.insert(stream.clone(), state);
        }
        for stream in &recovery.damaged {
            if let Some(state) = sender.streams.get_mut(stream) {
                // What was damaged may have been sent: the system is told that
                // events may be gone, and resyncs.
                state.reset_owed = Some((state.outbox.oldest(), ResetReason::SpoolLost));
            }
        }
        (sender, recovery)
    }

    fn state_from_spool(&self, stream: &str, now: Instant) -> StreamState {
        let next = self.spool.sent_through(stream) + 1;
        let outbox = Outbox::restored(next, self.spool.oldest(stream));
        StreamState {
            next_to_wire: outbox.oldest(),
            unacknowledged_bytes: 0,
            reset_owed: None,
            reset_sent: false,
            last_progress: now,
            outbox,
        }
    }

    fn state(&mut self, stream: &str, now: Instant) -> &mut StreamState {
        if !self.streams.contains_key(stream) {
            let state = StreamState {
                outbox: Outbox::new(),
                next_to_wire: 1,
                unacknowledged_bytes: 0,
                reset_owed: None,
                reset_sent: false,
                last_progress: now,
            };
            self.streams.insert(stream.to_owned(), state);
        }
        self.streams
            .get_mut(stream)
            .unwrap_or_else(|| unreachable!("the stream was just inserted"))
    }

    /// The streams that hold events.
    #[must_use]
    pub fn streams(&self) -> Vec<String> {
        self.spool.streams()
    }

    /// Puts an event into the spool. The event is sent by a later call to
    /// [`StreamSender::pump`]; returns frames that must go first when space
    /// had to be freed and the system must be told.
    pub fn publish(
        &mut self,
        stream: &Stream,
        at: Time,
        kind: &str,
        body: Json,
        now: Instant,
    ) -> Result<Vec<Frame>, PublishError> {
        if let Some(role) = stream.family().source()
            && !self.roles.contains(&role)
        {
            return Err(PublishError::RoleMissing(role, stream.to_string()));
        }
        let name = stream.to_string();
        let event = SpoolEvent {
            at,
            kind: kind.to_owned(),
            body,
        };
        let mut body_for_size = event.body.clone();
        if kind == "raw.frame"
            && let Some(body) = body_for_size.as_object_mut()
        {
            // The allocator fills n at append. Reserve the longest possible number on the wire.
            body.insert("n".to_owned(), Json::from(Seq::MAX));
        }
        let framed = serde_json::to_string(&EvbEntry {
            seq: Seq::MAX,
            at,
            kind: event.kind.clone(),
            body: body_for_size,
        })
        .map_or(usize::MAX, |text| text.len());
        let bytes = Evb::frame_bytes(&name, &[framed]);
        if bytes > self.max_frame_bytes {
            return Err(PublishError::TooLarge {
                bytes,
                max: self.max_frame_bytes,
            });
        }
        self.state(&name, now);
        let relief = self.spool.append(&name, &event)?;
        Ok(self.after_relief(relief, now))
    }

    /// Initializes a segment's capture allocator from the server while preserving queued numbers.
    pub fn raw_after(&mut self, stream: &Stream, after: Seq) -> Result<(), PublishError> {
        self.spool.raw_after(&stream.to_string(), after)?;
        Ok(())
    }

    /// Notes what a full spool gave up, and returns the frames that tell the
    /// system when it is reachable.
    fn after_relief(&mut self, relief: Vec<Relief>, now: Instant) -> Vec<Frame> {
        for step in relief {
            if let Relief::OldestEvents {
                stream,
                numbered_through,
                reason,
                ..
            } = step
            {
                let oldest = self.spool.oldest(&stream);
                let state = self.state(&stream, now);
                if let Some(through) = numbered_through {
                    state.outbox.dropped(through, reason);
                } else {
                    // Events that never had a number: the stream goes on from the
                    // number the next will take.
                    state.outbox = Outbox::restored(state.outbox.next(), state.outbox.next());
                }
                state.reset_owed = Some((oldest.max(state.outbox.oldest()), reason));
                state.reset_sent = false;
                state.next_to_wire = state.next_to_wire.max(oldest);
            }
        }
        self.owed_resets()
    }

    fn owed_resets(&mut self) -> Vec<Frame> {
        if !self.connected {
            return Vec::new();
        }
        let mut frames = Vec::new();
        for (stream, state) in &mut self.streams {
            if let (Some((from, reason)), false) = (state.reset_owed, state.reset_sent) {
                frames.push(Frame::Reset(Reset {
                    stream: stream.clone(),
                    from,
                    reason,
                }));
                // It stays owed until the system shows that it took it: an
                // acknowledgement at or past it, or a cursor that is.
                state.reset_sent = true;
            }
        }
        frames
    }

    /// A session became ready, and the system says how far it has taken each
    /// stream in. Returns what to send first.
    pub fn on_ready(
        &mut self,
        cursors: &BTreeMap<String, Seq>,
        max_frame_bytes: usize,
        now: Instant,
    ) -> Result<Vec<Frame>, SpoolError> {
        self.connected = true;
        self.max_frame_bytes = max_frame_bytes;
        let mut frames = Vec::new();
        let names: BTreeSet<String> = self
            .streams
            .keys()
            .chain(cursors.keys().filter(|name| name.parse::<Stream>().is_ok()))
            .cloned()
            .collect();
        for name in names {
            let cursor = cursors.get(&name).copied();
            let (resume, acknowledged) = {
                let state = self.state(&name, now);
                let resume = state.outbox.on_ready(cursor);
                (resume, state.outbox.acknowledged())
            };
            // A lost spool must advance its durable delivery allocator along with the outbox.
            self.spool.resume_after(&name, acknowledged)?;
            self.spool.acknowledge(&name, acknowledged)?;
            self.recount_unacknowledged(&name)?;
            let state = self.state(&name, now);
            state.last_progress = now;
            state.reset_sent = false;
            match resume {
                Resume::Send { from } => {
                    state.next_to_wire = from;
                    // A system that has taken in events from the number a reset
                    // goes on from has been told: the reset came before them.
                    if state
                        .reset_owed
                        .is_some_and(|(owed, _)| cursor.unwrap_or(0) >= owed)
                    {
                        state.reset_owed = None;
                    }
                }
                Resume::Reset { from, reason } => {
                    state.next_to_wire = from;
                    state.reset_owed = Some((from, reason));
                }
            }
            if let Some((from, reason)) = state.reset_owed {
                state.reset_sent = true;
                frames.push(Frame::Reset(Reset {
                    stream: name.clone(),
                    from,
                    reason,
                }));
            }
        }
        Ok(frames)
    }

    /// The session is gone.
    pub fn on_session_down(&mut self) {
        self.connected = false;
    }

    fn recount_unacknowledged(&mut self, stream: &str) -> Result<(), SpoolError> {
        let oldest = self.spool.oldest(stream);
        let sent_through = self.spool.sent_through(stream);
        let count = usize::try_from((sent_through - oldest + 1).max(0)).unwrap_or(0);
        let bytes: usize = self.spool.sizes(stream, oldest, count).iter().sum();
        if let Some(state) = self.streams.get_mut(stream) {
            state.unacknowledged_bytes = i64::try_from(bytes).unwrap_or(i64::MAX);
        }
        Ok(())
    }

    /// The system took a stream in through a number.
    pub fn on_ack(&mut self, ack: &Ack, now: Instant) -> Result<(), SpoolError> {
        let Some(state) = self.streams.get_mut(&ack.stream) else {
            return Ok(());
        };
        let through = state.outbox.on_ack(ack.seq);
        state.last_progress = now;
        state.next_to_wire = state.next_to_wire.max(through + 1);
        if state.reset_owed.is_some_and(|(owed, _)| through >= owed) {
            state.reset_owed = None;
        }
        self.spool.acknowledge(&ack.stream, through)?;
        self.recount_unacknowledged(&ack.stream)
    }

    /// The system says events are missing and wants the stream again from a
    /// number. Returns a reset when the machine no longer holds them.
    pub fn on_nak(&mut self, nak: &Nak, now: Instant) -> Result<Vec<Frame>, SpoolError> {
        let Some(state) = self.streams.get_mut(&nak.stream) else {
            return Ok(Vec::new());
        };
        state.last_progress = now;
        let resume = state.outbox.on_nak(nak.expected);
        let acknowledged = state.outbox.acknowledged();
        let mut frames = Vec::new();
        match resume {
            Resume::Send { from } => state.next_to_wire = from,
            Resume::Reset { from, reason } => {
                state.next_to_wire = from;
                state.reset_owed = Some((from, reason));
                state.reset_sent = true;
                frames.push(Frame::Reset(Reset {
                    stream: nak.stream.clone(),
                    from,
                    reason,
                }));
            }
        }
        self.spool.acknowledge(&nak.stream, acknowledged)?;
        self.recount_unacknowledged(&nak.stream)?;
        Ok(frames)
    }

    /// Notes that a stream has gone a long time with events on the wire and
    /// no word from the system, and sends them again.
    pub fn on_stall(&mut self, after: Duration, now: Instant) {
        for (stream, state) in &mut self.streams {
            let outstanding = state.outbox.acknowledged() < self.spool.sent_through(stream);
            let all_on_the_wire = state.next_to_wire > self.spool.sent_through(stream);
            if outstanding && all_on_the_wire && now.duration_since(state.last_progress) >= after {
                state.next_to_wire = state.outbox.acknowledged() + 1;
                state.last_progress = now;
            }
        }
    }

    /// Whether any stream has events on the wire that the system has not
    /// acknowledged.
    #[must_use]
    pub fn has_unacknowledged(&self) -> bool {
        self.streams
            .iter()
            .any(|(stream, state)| state.outbox.acknowledged() < self.spool.sent_through(stream))
    }

    /// How a stream stands.
    #[must_use]
    pub fn pressure(&self, stream: &Stream) -> Pressure {
        let name = stream.to_string();
        self.streams.get(&name).map_or(
            Pressure {
                unacknowledged_events: 0,
                unacknowledged_bytes: 0,
                waiting_events: 0,
                blocked: false,
            },
            |state| Pressure {
                unacknowledged_events: state.outbox.unacknowledged(),
                unacknowledged_bytes: state.unacknowledged_bytes,
                waiting_events: self.spool.unsent(&name),
                blocked: !state.outbox.may_send(state.unacknowledged_bytes),
            },
        )
    }

    /// Frames everything that may go on the wire now: what the system must be
    /// sent again, then new events while the window has room, then, when
    /// nothing else waits anywhere, a few diagnostics. Does nothing while no
    /// session is up.
    pub fn pump(&mut self, frame_bytes: usize, now: Instant) -> Result<Vec<Frame>, SpoolError> {
        if !self.connected {
            return Ok(Vec::new());
        }
        let frame_bytes = frame_bytes.min(self.max_frame_bytes);
        let mut frames = Vec::new();
        let names: Vec<String> = self.streams.keys().cloned().collect();
        for name in &names {
            self.pump_stream(name, frame_bytes, now, &mut frames)?;
        }
        if frames.is_empty() && !self.anything_waits() {
            let mut promoted = false;
            for name in &names {
                if self.spool.diagnostics_waiting(name) > 0 {
                    self.spool.promote_diagnostics(name, DIAGNOSTIC_BATCH)?;
                    promoted = true;
                    break;
                }
            }
            if promoted {
                for name in &names {
                    self.pump_stream(name, frame_bytes, now, &mut frames)?;
                }
            }
        }
        Ok(frames)
    }

    fn anything_waits(&self) -> bool {
        self.spool.anything_unsent()
            || self
                .streams
                .iter()
                .any(|(stream, state)| state.next_to_wire <= self.spool.sent_through(stream))
    }

    fn pump_stream(
        &mut self,
        name: &str,
        frame_bytes: usize,
        now: Instant,
        frames: &mut Vec<Frame>,
    ) -> Result<(), SpoolError> {
        loop {
            // What was numbered and not put on the wire yet, or must go again.
            let sent_through = self.spool.sent_through(name);
            let next_to_wire = self.streams.get(name).map_or(1, |state| state.next_to_wire);
            let mut batch: Vec<Numbered> = Vec::new();
            if next_to_wire <= sent_through {
                batch = self.spool.read(name, next_to_wire, READ_BATCH)?;
            } else if self.spool.unsent(name) > 0 {
                let Some(state) = self.streams.get(name) else {
                    return Ok(());
                };
                if !state.outbox.may_send(state.unacknowledged_bytes) {
                    return Ok(());
                }
                // Take as many as the window has room for, no more.
                let room_events = usize::try_from(
                    orch_link_contract::streams::MAX_UNACKNOWLEDGED_EVENTS
                        - state.outbox.unacknowledged(),
                )
                .unwrap_or(0);
                batch = self.spool.number(name, room_events.min(READ_BATCH))?;
                for numbered in &batch {
                    let bytes =
                        serde_json::to_string(&entry_of(numbered)).map_or(0, |text| text.len());
                    if let Some(state) = self.streams.get_mut(name) {
                        let seq = state.outbox.number();
                        debug_assert_eq!(seq, numbered.seq, "the outbox and the spool count alike");
                        state.unacknowledged_bytes += i64::try_from(bytes).unwrap_or(i64::MAX);
                    }
                }
            }
            if batch.is_empty() {
                return Ok(());
            }
            // The numbers are on disk before any of them is on the wire.
            let entries: Vec<EvbEntry> = batch.iter().map(entry_of).collect();
            let sizes: Vec<usize> = entries
                .iter()
                .map(|entry| serde_json::to_string(entry).map_or(usize::MAX, |text| text.len()))
                .collect();
            let mut placed = 0;
            while placed < entries.len() {
                let fit = Evb::fitting(name, &sizes[placed..], frame_bytes.max(1));
                // A single event is always sent, even in a frame larger than
                // the one a thin link prefers: it cannot be cut.
                let fit = if fit == 0 {
                    Evb::fitting(name, &sizes[placed..], self.max_frame_bytes).max(1)
                } else {
                    fit
                };
                let slice = &entries[placed..placed + fit];
                frames.push(if let [only] = slice {
                    Frame::Ev(Ev {
                        stream: name.to_owned(),
                        seq: only.seq,
                        at: only.at,
                        kind: only.kind.clone(),
                        body: only.body.clone(),
                    })
                } else {
                    Frame::Evb(Evb {
                        stream: name.to_owned(),
                        events: slice.to_vec(),
                    })
                });
                placed += fit;
            }
            if let Some(state) = self.streams.get_mut(name) {
                state.next_to_wire = batch.last().map_or(state.next_to_wire, |last| last.seq + 1);
                state.last_progress = now;
            }
        }
    }

    /// Files to force to disk; the caller does it where blocking is fine.
    pub fn sync_handles(&mut self) -> Result<Vec<std::fs::File>, SpoolError> {
        self.spool.flush_for_sync()
    }

    /// Whether something was spooled since the last sync.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.spool.is_dirty()
    }

    /// Drops what is older than the spool keeps, and returns the frames that
    /// tell the system.
    pub fn expire(&mut self, now: Time, instant: Instant) -> Result<Vec<Frame>, SpoolError> {
        let relief = self.spool.expire(now)?;
        Ok(self.after_relief(relief, instant))
    }

    /// Forces everything spooled to disk, here and now.
    pub fn sync(&mut self) -> Result<(), SpoolError> {
        self.spool.sync()
    }

    /// Whether a kind is a diagnostic.
    #[must_use]
    pub fn is_diagnostic(kind: &str) -> bool {
        is_diagnostic(kind)
    }
}

// ---------------------------------------------------------------- the system's side

/// One event that is to be applied.
#[derive(Debug, Clone, PartialEq)]
pub struct DeliveredEvent {
    /// Its number in its stream.
    pub seq: Seq,
    /// When it happened.
    pub at: Time,
    /// Its kind.
    pub kind: String,
    /// Its body.
    pub body: Json,
}

/// The new events of one frame: to be applied in order, in one
/// transaction, with the stream's cursor set to [`Delivery::last_seq`].
#[derive(Debug)]
pub struct Delivery {
    /// The stream.
    pub stream: Stream,
    /// The events to apply, in order. Never empty.
    pub events: Vec<DeliveredEvent>,
    /// The cursor to store with them.
    pub last_seq: Seq,
    plan: orch_link_contract::streams::Plan,
}

/// What the system does with a frame of events.
#[derive(Debug)]
pub enum Received {
    /// Apply these, then tell the receiver they were committed.
    Apply(Delivery),
    /// Nothing to apply: send this answer at once.
    Answer(Box<Frame>),
    /// Another frame of the same stream is being applied; this one is kept
    /// until it is done.
    Queued,
    /// The frame is not acceptable and is dropped.
    Refused(String),
}

/// A machine's word that events are gone, as the system takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetNotice {
    /// The stream.
    pub stream: Stream,
    /// The number the stream goes on from.
    pub from: Seq,
    /// Why.
    pub reason: ResetReason,
    /// Whether the system should ask the machine's adapter to report the
    /// session again from its own history.
    pub resync: bool,
}

struct ReceiverStream {
    stream: Stream,
    inbox: Inbox,
    applying: bool,
    waiting: VecDeque<Vec<EvbEntry>>,
}

/// The system's side of the streams of one machine.
pub struct StreamReceiver {
    streams: HashMap<String, ReceiverStream>,
    cursors: BTreeMap<String, Seq>,
    roles: Vec<Role>,
}

impl std::fmt::Debug for StreamReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamReceiver")
            .field("streams", &self.streams.len())
            .finish_non_exhaustive()
    }
}

impl StreamReceiver {
    /// A receiver for a machine with `roles`, whose streams the system has
    /// taken in as far as `cursors` say.
    #[must_use]
    pub fn new(cursors: BTreeMap<String, Seq>, roles: Vec<Role>) -> Self {
        Self {
            streams: HashMap::new(),
            cursors,
            roles,
        }
    }

    /// How far each stream was taken in, as a gateway states it to a machine
    /// that becomes ready.
    #[must_use]
    pub fn cursors(&self) -> BTreeMap<String, Seq> {
        let mut cursors = self.cursors.clone();
        for (name, state) in &self.streams {
            cursors.insert(name.clone(), state.inbox.last_seq());
        }
        cursors
    }

    /// Changes the roles the machine has.
    pub fn set_roles(&mut self, roles: Vec<Role>) {
        self.roles = roles;
    }

    fn stream_state(&mut self, name: &str) -> Result<&mut ReceiverStream, String> {
        if !self.streams.contains_key(name) {
            let stream: Stream = name.parse().map_err(|error| format!("{error}"))?;
            if let Some(role) = stream.family().source()
                && !self.roles.contains(&role)
            {
                return Err(format!(
                    "the machine has not the role {role:?} that reports on {name}"
                ));
            }
            let last_seq = self.cursors.get(name).copied().unwrap_or(0);
            self.streams.insert(
                name.to_owned(),
                ReceiverStream {
                    stream,
                    inbox: Inbox::new(last_seq),
                    applying: false,
                    waiting: VecDeque::new(),
                },
            );
        }
        self.streams
            .get_mut(name)
            .ok_or_else(|| "the stream is not known".to_owned())
    }

    /// A single event.
    pub fn on_ev(&mut self, ev: Ev) -> Received {
        let name = ev.stream.clone();
        self.take(
            &name,
            vec![EvbEntry {
                seq: ev.seq,
                at: ev.at,
                kind: ev.kind,
                body: ev.body,
            }],
        )
    }

    /// A batch of events.
    pub fn on_evb(&mut self, evb: Evb) -> Received {
        let name = evb.stream.clone();
        self.take(&name, evb.events)
    }

    fn take(&mut self, name: &str, entries: Vec<EvbEntry>) -> Received {
        let state = match self.stream_state(name) {
            Ok(state) => state,
            Err(why) => return Received::Refused(why),
        };
        if state.applying {
            state.waiting.push_back(entries);
            return Received::Queued;
        }
        Self::plan(state, entries)
    }

    fn plan(state: &mut ReceiverStream, entries: Vec<EvbEntry>) -> Received {
        let numbers: Vec<Seq> = entries.iter().map(|entry| entry.seq).collect();
        let plan = state.inbox.plan(&numbers);
        if plan.apply.is_empty() {
            return Received::Answer(Box::new(Self::answer_frame(&state.stream, &plan)));
        }
        let events = plan
            .apply
            .iter()
            .filter_map(|place| entries.get(*place))
            .map(|entry| DeliveredEvent {
                seq: entry.seq,
                at: entry.at,
                kind: entry.kind.clone(),
                body: entry.body.clone(),
            })
            .collect();
        state.applying = true;
        Received::Apply(Delivery {
            stream: state.stream,
            events,
            last_seq: plan.last_seq,
            plan,
        })
    }

    fn answer_frame(stream: &Stream, plan: &orch_link_contract::streams::Plan) -> Frame {
        match plan.answer {
            Answer::Ack(through) => Frame::Ack(Ack {
                stream: stream.to_string(),
                seq: through,
            }),
            Answer::Nak(expected) => Frame::Nak(Nak {
                stream: stream.to_string(),
                expected,
            }),
        }
    }

    /// The caller committed a delivery. Returns the answer to send, and, when
    /// frames were waiting behind it, what to do with them (`next_waiting`).
    pub fn committed(&mut self, delivery: &Delivery) -> (Frame, Option<Received>) {
        let name = delivery.stream.to_string();
        let answer = Self::answer_frame(&delivery.stream, &delivery.plan);
        let Some(state) = self.streams.get_mut(&name) else {
            return (answer, None);
        };
        state.inbox.committed(&delivery.plan);
        state.applying = false;
        let next = Self::next_waiting(state);
        (answer, next)
    }

    /// The caller could not commit a delivery. Nothing is answered: the
    /// machine sends the events again when it reconnects or when it
    /// stalls. Returns what to do with the frames waiting behind it, if any
    /// (`next_waiting`), judged against the unchanged cursor.
    pub fn abandoned(&mut self, delivery: &Delivery) -> Option<Received> {
        let name = delivery.stream.to_string();
        let state = self.streams.get_mut(&name)?;
        state.applying = false;
        Self::next_waiting(state)
    }

    /// The frames that waited behind a delivery, judged together as one frame (C-turns), in the order
    /// they came. A frame is joined to those before it while together they leave no hole, up to
    /// [`READ_BATCH`] events, which is what the machine frames from its spool at a time and so a delivery
    /// the system takes already; a frame that would leave a hole is judged on its own, as it came, so
    /// that what fills the hole after it is not lost with it. Frames with nothing to apply (sent again by
    /// a machine that heard nothing for a while, or out of their place) do not hold up those behind them:
    /// the first with something to apply are the next delivery, and where none has, the machine is
    /// answered once, asked again from a hole if one was found. One frame to a transaction, a stream that
    /// had fallen behind never caught up: a hundred sessions of one machine, each waiting for one of a
    /// few transactions at a time on a busy pool, had their turns' starts heard minutes late and given
    /// up; and a frame of what had come already was answered while the frames behind it waited for the
    /// machine's next.
    fn next_waiting(state: &mut ReceiverStream) -> Option<Received> {
        let mut answer: Option<Received> = None;
        while let Some(mut entries) = state.waiting.pop_front() {
            let mut through = Self::reach(state.inbox.last_seq(), &entries);
            while let (Some(now), Some(following)) = (through, state.waiting.front()) {
                let then = Self::reach(now, following);
                if then.is_none() || entries.len() + following.len() > READ_BATCH {
                    break;
                }
                through = then;
                if let Some(following) = state.waiting.pop_front() {
                    entries.extend(following);
                }
            }
            match Self::plan(state, entries) {
                Received::Apply(delivery) => return Some(Received::Apply(delivery)),
                judged => {
                    if answer.as_ref().is_none_or(|kept| !Self::asks_again(kept))
                        || Self::asks_again(&judged)
                    {
                        answer = Some(judged);
                    }
                }
            }
        }
        answer
    }

    /// How far a stream taken in through `through` goes with `entries`, judged as [`Inbox::plan`] judges
    /// them; none where they leave a hole.
    fn reach(mut through: Seq, entries: &[EvbEntry]) -> Option<Seq> {
        for entry in entries {
            if entry.seq > through.saturating_add(1) {
                return None;
            }
            through = through.max(entry.seq);
        }
        Some(through)
    }

    /// Whether a judgement asks the machine to send its stream again from a hole.
    fn asks_again(received: &Received) -> bool {
        matches!(received, Received::Answer(frame) if matches!(frame.as_ref(), Frame::Nak(_)))
    }

    /// A machine's word that events of a stream are gone.
    pub fn on_reset(&mut self, reset: &Reset) -> Result<ResetNotice, String> {
        let state = self.stream_state(&reset.stream)?;
        state.inbox.on_reset(reset.from);
        Ok(ResetNotice {
            stream: state.stream,
            from: reset.from,
            reason: reset.reason,
            resync: state.stream.resyncs_after_reset(),
        })
    }

    /// A measurement that is not kept: checked against the roles, and
    /// returned to be passed on.
    pub fn on_eph(&mut self, eph: Eph) -> Result<Eph, String> {
        self.stream_state(&eph.stream)?;
        Ok(eph)
    }
}

#[cfg(test)]
#[allow(clippy::panic, reason = "a test fails by panicking")]
mod receiver_tests {
    use super::*;

    fn session() -> Stream {
        Stream::Session {
            thread_id: orch_types::new_id(),
            segment_id: orch_types::new_id(),
        }
    }

    fn receiver(stream: &Stream) -> StreamReceiver {
        StreamReceiver::new(
            BTreeMap::new(),
            stream.family().source().into_iter().collect(),
        )
    }

    fn frame(stream: &Stream, numbers: std::ops::RangeInclusive<Seq>) -> Evb {
        Evb {
            stream: stream.to_string(),
            events: numbers
                .map(|seq| EvbEntry {
                    seq,
                    at: Time::new(chrono::Utc::now()),
                    kind: "session.title".to_owned(),
                    body: serde_json::json!({ "title": format!("title {seq}") }),
                })
                .collect(),
        }
    }

    fn applied(received: Option<Received>) -> Delivery {
        match received {
            Some(Received::Apply(delivery)) => delivery,
            other => panic!("a delivery was expected, not {other:?}"),
        }
    }

    fn numbers(delivery: &Delivery) -> Vec<Seq> {
        delivery.events.iter().map(|event| event.seq).collect()
    }

    #[test]
    fn frames_that_waited_behind_a_delivery_are_taken_in_together_in_the_order_they_came() {
        let stream = session();
        let mut receiver = receiver(&stream);
        let first = applied(Some(receiver.on_evb(frame(&stream, 1..=2))));
        assert!(matches!(
            receiver.on_evb(frame(&stream, 3..=3)),
            Received::Queued
        ));
        assert!(matches!(
            receiver.on_evb(frame(&stream, 4..=6)),
            Received::Queued
        ));
        // Sent again after a stall: what came already is passed over.
        assert!(matches!(
            receiver.on_evb(frame(&stream, 5..=7)),
            Received::Queued
        ));
        // After a hole (8 never came): judged on its own, as it came.
        assert!(matches!(
            receiver.on_evb(frame(&stream, 9..=9)),
            Received::Queued
        ));

        let (answer, next) = receiver.committed(&first);
        assert!(matches!(answer, Frame::Ack(Ack { seq: 2, .. })));
        let joined = applied(next);
        assert_eq!(numbers(&joined), vec![3, 4, 5, 6, 7]);
        assert_eq!(joined.last_seq, 7);

        let (answer, next) = receiver.committed(&joined);
        assert!(matches!(answer, Frame::Ack(Ack { seq: 7, .. })));
        assert!(matches!(
            next,
            Some(Received::Answer(frame)) if matches!(frame.as_ref(), Frame::Nak(Nak { expected: 8, .. }))
        ));
    }

    #[test]
    fn frames_taken_in_together_are_no_more_than_the_machine_frames_at_a_time() {
        let stream = session();
        let mut receiver = receiver(&stream);
        let first = applied(Some(receiver.on_evb(frame(&stream, 1..=1))));
        assert!(matches!(
            receiver.on_evb(frame(&stream, 2..=200)),
            Received::Queued
        ));
        assert!(matches!(
            receiver.on_evb(frame(&stream, 201..=300)),
            Received::Queued
        ));

        // 199 and 100 events are more than READ_BATCH (256) together: each is a delivery of its own.
        let (_, next) = receiver.committed(&first);
        let second = applied(next);
        assert_eq!(second.events.len(), 199);
        assert_eq!(second.last_seq, 200);
        let (_, next) = receiver.committed(&second);
        let third = applied(next);
        assert_eq!(numbers(&third).first(), Some(&201));
        assert_eq!(third.last_seq, 300);

        // An abandoned delivery leaves the stream where it was: what waited behind it is judged against
        // that, and the machine is asked for the stream again from there.
        assert!(matches!(
            receiver.on_evb(frame(&stream, 301..=301)),
            Received::Queued
        ));
        assert!(matches!(
            receiver.on_evb(frame(&stream, 302..=303)),
            Received::Queued
        ));
        assert!(matches!(
            receiver.abandoned(&third),
            Some(Received::Answer(frame)) if matches!(frame.as_ref(), Frame::Nak(Nak { expected: 201, .. }))
        ));
    }

    #[test]
    fn frames_of_what_came_already_or_out_of_their_place_do_not_hold_up_the_frames_behind_them() {
        let stream = session();
        let mut receiver = receiver(&stream);
        let first = applied(Some(receiver.on_evb(frame(&stream, 1..=7))));
        // Sent again by a machine that heard nothing for a while, then what came next.
        assert!(matches!(
            receiver.on_evb(frame(&stream, 2..=3)),
            Received::Queued
        ));
        assert!(matches!(
            receiver.on_evb(frame(&stream, 8..=8)),
            Received::Queued
        ));
        let (_, next) = receiver.committed(&first);
        let eighth = applied(next);
        assert_eq!(numbers(&eighth), vec![8]);

        // A frame out of its place (10 before 9), then the stream sent again from the hole: the frame
        // after the hole is not joined to the one before it, so what fills the hole is not lost.
        assert!(matches!(
            receiver.on_evb(frame(&stream, 10..=10)),
            Received::Queued
        ));
        assert!(matches!(
            receiver.on_evb(frame(&stream, 9..=11)),
            Received::Queued
        ));
        let (_, next) = receiver.committed(&eighth);
        assert_eq!(numbers(&applied(next)), vec![9, 10, 11]);
    }

    #[test]
    fn where_nothing_waiting_is_applied_the_machine_is_asked_again_from_the_hole() {
        let stream = session();
        let mut receiver = receiver(&stream);
        let first = applied(Some(receiver.on_evb(frame(&stream, 1..=8))));
        assert!(matches!(
            receiver.on_evb(frame(&stream, 10..=10)),
            Received::Queued
        ));
        assert!(matches!(
            receiver.on_evb(frame(&stream, 5..=6)),
            Received::Queued
        ));
        let (_, next) = receiver.committed(&first);
        assert!(matches!(
            next,
            Some(Received::Answer(frame)) if matches!(frame.as_ref(), Frame::Nak(Nak { expected: 9, .. }))
        ));
    }
}
