//! Durable streams, acknowledgement and reset.
//!
//! What a machine has to tell the system travels upward in streams. Each
//! stream numbers its events from one, without gaps. The machine keeps an
//! event until the system says that it has taken it in; the system takes
//! events in strictly in order, each exactly once, and answers with how
//! far it has come. After a broken session the machine sends again from
//! where the system says it stands, so nothing is lost and nothing is
//! applied twice. When a machine no longer holds events the system has
//! not seen, it says so with a reset, and the system knows that there is
//! a hole to repair.
//!
//! Measurements that are worthless when they are late travel beside the
//! streams: they are not kept, not acknowledged and not sent again.
//!
//! [`Outbox`] is the arithmetic of the sending side and [`Inbox`] that of
//! the receiving side. Neither stores an event.

use crate::session::Role;
use orch_types::revision::{self, arrival};
use orch_types::{Id, Json, Seq, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The number of a stream's first event.
pub const FIRST_SEQ: Seq = 1;
/// The most events of one stream that may be on their way without an
/// acknowledgement. Beyond it the machine stops reading from whatever
/// produces them, so that a system that has fallen behind is not buried.
pub const MAX_UNACKNOWLEDGED_EVENTS: i64 = 1_000;
/// The most bytes of one stream that may be on their way without an
/// acknowledgement: a few seconds of a slow link's rate.
pub const MAX_UNACKNOWLEDGED_BYTES: i64 = 4 * 1024 * 1024;
/// The size of one file of a machine's spool.
pub const SPOOL_SEGMENT_BYTES: i64 = 8 * 1024 * 1024;
/// The longest a spooled event waits before it is forced to disk. A
/// fifth of a second bounds what a crash can lose without paying for a
/// sync on every event.
pub const SPOOL_SYNC_MILLIS: i64 = 200;
/// The most bytes a machine's spool holds before it frees space.
pub const SPOOL_MAX_BYTES: i64 = 256 * 1024 * 1024;
/// The oldest event a machine's spool holds, in days: long enough for a
/// machine that was away over a weekend and a holiday.
pub const SPOOL_MAX_AGE_DAYS: i64 = 7;
/// How the kinds of events begin that are diagnostics: they are sent
/// only when no other event of any stream waits.
pub const DIAGNOSTIC_PREFIX: &str = "raw.";

/// Whether events of `kind` are diagnostics, which give way to every
/// other event and are the first to go when a spool is full.
#[must_use]
pub fn is_diagnostic(kind: &str) -> bool {
    kind.starts_with(DIAGNOSTIC_PREFIX)
}

/// What a machine gives up, and in which order, when its spool is full.
/// It frees space in this order and in no other way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpoolRelief {
    /// Diagnostics.
    Diagnostics,
    /// The growing text of an item whose completed form is spooled as
    /// well, and carries the whole text.
    SupersededDeltas,
    /// The oldest events of the fullest stream; the system is then told
    /// with a reset.
    OldestEvents,
}

impl SpoolRelief {
    /// The order in which space is freed.
    pub const ORDER: [SpoolRelief; 3] = [
        Self::Diagnostics,
        Self::SupersededDeltas,
        Self::OldestEvents,
    ];
}

/// The kind of thing a stream reports about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamFamily {
    /// The machine itself.
    Machine,
    /// A harness process.
    Harness,
    /// A provider.
    Provider,
    /// An environment.
    Env,
    /// A thread's session.
    Session,
    /// A host's node.
    Node,
}

impl StreamFamily {
    /// The word a stream of this family begins with.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Machine => "machine",
            Self::Harness => "harness",
            Self::Provider => "provider",
            Self::Env => "env",
            Self::Session => "session",
            Self::Node => "node",
        }
    }

    /// The role a machine needs to send on such a stream; `None` where
    /// any machine may.
    #[must_use]
    pub const fn source(self) -> Option<Role> {
        match self {
            Self::Machine => None,
            Self::Harness | Self::Session => Some(Role::Agent),
            Self::Provider => Some(Role::Provider),
            Self::Env => Some(Role::Environment),
            Self::Node => Some(Role::Node),
        }
    }
}

/// A stream of one machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stream {
    /// The machine's facts, the state of its roles, its local policy.
    Machine,
    /// The state and the descriptor of one harness process.
    Harness {
        /// The provider it serves.
        provider_id: Id,
        /// The agent profile it runs.
        agent_id: Id,
    },
    /// A provider's state, usage, sign-in and leases.
    Provider {
        /// The provider.
        provider_id: Id,
    },
    /// An environment's state, facts, file lists and jobs.
    Env {
        /// The environment.
        environment_id: Id,
    },
    /// Everything a harness session does.
    Session {
        /// The thread.
        thread_id: Id,
        /// The stretch of the thread the session is.
        segment_id: Id,
    },
    /// A host's reports about its units and itself.
    Node,
}

impl Stream {
    /// The kind of thing the stream reports about.
    #[must_use]
    pub const fn family(&self) -> StreamFamily {
        match self {
            Self::Machine => StreamFamily::Machine,
            Self::Harness { .. } => StreamFamily::Harness,
            Self::Provider { .. } => StreamFamily::Provider,
            Self::Env { .. } => StreamFamily::Env,
            Self::Session { .. } => StreamFamily::Session,
            Self::Node => StreamFamily::Node,
        }
    }

    /// Whether a hole in the stream is repaired by asking the adapter to
    /// report its session again from the harness's own history.
    #[must_use]
    pub const fn resyncs_after_reset(&self) -> bool {
        matches!(self, Self::Session { .. })
    }
}

impl std::fmt::Display for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let prefix = self.family().prefix();
        match self {
            Self::Machine | Self::Node => f.write_str(prefix),
            Self::Harness {
                provider_id,
                agent_id,
            } => write!(f, "{prefix}:{provider_id}:{agent_id}"),
            Self::Provider { provider_id } => write!(f, "{prefix}:{provider_id}"),
            Self::Env { environment_id } => write!(f, "{prefix}:{environment_id}"),
            Self::Session {
                thread_id,
                segment_id,
            } => write!(f, "{prefix}:{thread_id}:{segment_id}"),
        }
    }
}

/// A text that names no stream.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not the name of a stream")]
pub struct NotAStream(pub String);

/// An identifier written the one way identifiers are written: lowercase,
/// with hyphens. A stream has one name, so that it has one cursor.
fn canonical_id(text: &str) -> Option<Id> {
    text.parse::<Id>().ok().filter(|id| id.to_string() == text)
}

impl std::str::FromStr for Stream {
    type Err = NotAStream;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bad = || NotAStream(text.to_owned());
        let mut parts = text.split(':');
        let family = parts.next().ok_or_else(bad)?;
        let mut id = || parts.next().and_then(canonical_id).ok_or_else(bad);
        let stream = match family {
            "machine" => Self::Machine,
            "node" => Self::Node,
            "harness" => Self::Harness {
                provider_id: id()?,
                agent_id: id()?,
            },
            "provider" => Self::Provider { provider_id: id()? },
            "env" => Self::Env {
                environment_id: id()?,
            },
            "session" => Self::Session {
                thread_id: id()?,
                segment_id: id()?,
            },
            _ => return Err(bad()),
        };
        if parts.next().is_some() {
            return Err(bad());
        }
        Ok(stream)
    }
}

impl Serialize for Stream {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Stream {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Stream {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Stream".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string" })
    }
}

/// One event of a stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Ev {
    /// The stream.
    #[serde(rename = "s")]
    pub stream: String,
    /// The event's number in its stream.
    #[serde(rename = "q")]
    pub seq: Seq,
    /// When it happened.
    pub at: Time,
    /// Its kind.
    #[serde(rename = "k")]
    pub kind: String,
    /// Its body, as its kind defines it.
    #[serde(rename = "b")]
    pub body: Json,
}

/// One event of a batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EvbEntry {
    /// The event's number in its stream.
    #[serde(rename = "q")]
    pub seq: Seq,
    /// When it happened.
    pub at: Time,
    /// Its kind.
    #[serde(rename = "k")]
    pub kind: String,
    /// Its body, as its kind defines it.
    #[serde(rename = "b")]
    pub body: Json,
}

/// Several events of one stream that were ready together, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Evb {
    /// The stream.
    #[serde(rename = "s")]
    pub stream: String,
    /// The events.
    #[serde(rename = "e")]
    pub events: Vec<EvbEntry>,
}

/// The system's word that it has taken a stream in through a number.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Ack {
    /// The stream.
    #[serde(rename = "s")]
    pub stream: String,
    /// The last number taken in.
    #[serde(rename = "q")]
    pub seq: Seq,
}

/// The system's word that events are missing: it wants the stream again
/// from a number, and has everything before it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Nak {
    /// The stream.
    #[serde(rename = "s")]
    pub stream: String,
    /// The number it expects next.
    pub expected: Seq,
}

/// Why a machine no longer holds events the system has not seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResetReason {
    /// Its spool is gone or damaged.
    SpoolLost,
    /// Its spool was full, and the oldest events made room.
    SpoolOverflow,
}

/// A machine's word that a stream goes on from a number, and that
/// whatever lay between that and what the system has is gone.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Reset {
    /// The stream.
    #[serde(rename = "s")]
    pub stream: String,
    /// The number of the next event that will come.
    pub from: Seq,
    /// Why events are gone.
    pub reason: ResetReason,
}

/// A measurement that is worthless when it is late: not kept, not
/// acknowledged, not sent again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Eph {
    /// The stream it belongs beside.
    #[serde(rename = "s")]
    pub stream: String,
    /// Its kind.
    #[serde(rename = "k")]
    pub kind: String,
    /// When it was taken.
    pub at: Time,
    /// Its body, as its kind defines it.
    #[serde(rename = "b")]
    pub body: Json,
}

impl From<Ev> for EvbEntry {
    fn from(event: Ev) -> Self {
        Self {
            seq: event.seq,
            at: event.at,
            kind: event.kind,
            body: event.body,
        }
    }
}

impl Evb {
    /// How many bytes a batch of `stream` takes as a frame when its
    /// events take `event_bytes` each, written one by one.
    #[must_use]
    pub fn frame_bytes(stream: &str, event_bytes: &[usize]) -> usize {
        // {"t":"evb","s":<stream>,"e":[<event>,<event>]}
        const AROUND: usize = r#"{"t":"evb","s":,"e":[]}"#.len();
        // Every byte of a name could need the longest escape there is.
        const LONGEST_ESCAPE: usize = r"\u0000".len();
        let name = serde_json::to_string(stream)
            .map_or(stream.len() * LONGEST_ESCAPE + 2, |name| name.len());
        let commas = event_bytes.len().saturating_sub(1);
        AROUND + name + event_bytes.iter().sum::<usize>() + commas
    }

    /// How many of the events that wait, from the first, go into one
    /// frame of at most `max_frame_bytes`. Zero means that the first
    /// event alone is too large for any frame.
    #[must_use]
    pub fn fitting(stream: &str, event_bytes: &[usize], max_frame_bytes: usize) -> usize {
        (1..=event_bytes.len())
            .take_while(|count| {
                Self::frame_bytes(stream, &event_bytes[..*count]) <= max_frame_bytes
            })
            .last()
            .unwrap_or(0)
    }
}

/// What a machine does to bring the system up to date on a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Resume {
    /// Send the events it holds from this number on; there may be none.
    Send {
        /// The first number to send.
        from: Seq,
    },
    /// Say that events are gone, then send from this number on.
    Reset {
        /// The number the stream goes on from.
        from: Seq,
        /// Why events are gone.
        reason: ResetReason,
    },
}

/// One stream as the machine that sends it counts it. Events take their
/// numbers in the order they are put on the wire; a number, once given,
/// stays with its event however often it is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outbox {
    /// The number the next new event takes.
    next: Seq,
    /// The system has taken in everything through this number.
    acknowledged: Seq,
    /// The lowest number the machine still holds; `next` when it holds
    /// none.
    oldest: Seq,
    /// Why numbered events left the machine unacknowledged, if any did.
    gone: Option<ResetReason>,
}

impl Default for Outbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Outbox {
    /// A stream that has sent nothing yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next: FIRST_SEQ,
            acknowledged: FIRST_SEQ - 1,
            oldest: FIRST_SEQ,
            gone: None,
        }
    }

    /// A stream as a machine finds it when it starts: `next` is the
    /// number its next new event takes, and `oldest` the lowest number
    /// its spool still holds, or `next` when the spool holds none.
    #[must_use]
    pub fn restored(next: Seq, oldest: Seq) -> Self {
        let next = next.max(FIRST_SEQ);
        let oldest = oldest.clamp(FIRST_SEQ, next);
        Self {
            next,
            acknowledged: oldest - 1,
            oldest,
            gone: None,
        }
    }

    /// The number the next new event takes.
    #[must_use]
    pub const fn next(&self) -> Seq {
        self.next
    }

    /// The number through which the system has taken the stream in.
    #[must_use]
    pub const fn acknowledged(&self) -> Seq {
        self.acknowledged
    }

    /// The lowest number the machine still holds.
    #[must_use]
    pub const fn oldest(&self) -> Seq {
        self.oldest
    }

    /// How many numbered events wait for an acknowledgement.
    #[must_use]
    pub const fn unacknowledged(&self) -> i64 {
        self.next - self.oldest
    }

    /// Whether one more event may be numbered and sent, when those that
    /// wait for an acknowledgement take `unacknowledged_bytes` together.
    #[must_use]
    pub const fn may_send(&self, unacknowledged_bytes: i64) -> bool {
        self.unacknowledged() < MAX_UNACKNOWLEDGED_EVENTS
            && unacknowledged_bytes < MAX_UNACKNOWLEDGED_BYTES
    }

    /// Gives the next event its number.
    pub fn number(&mut self) -> Seq {
        let seq = self.next;
        self.next += 1;
        seq
    }

    /// Takes an acknowledgement. Returns the number through which the
    /// machine may delete what it holds.
    pub fn on_ack(&mut self, through: Seq) -> Seq {
        self.acknowledged = self.acknowledged.max(through.min(self.next - 1));
        if self.oldest <= self.acknowledged {
            self.oldest = self.acknowledged + 1;
            self.gone = None;
        }
        self.acknowledged
    }

    /// Records that the machine no longer holds its numbered events
    /// through `through`, though the system never acknowledged them.
    /// Returns what to tell the system, when anything it had not
    /// acknowledged is gone.
    pub fn dropped(&mut self, through: Seq, reason: ResetReason) -> Option<Resume> {
        let through = through.min(self.next - 1);
        if through < self.oldest {
            return None;
        }
        self.oldest = through + 1;
        self.gone = Some(reason);
        Some(Resume::Reset {
            from: self.oldest,
            reason,
        })
    }

    /// What to do when a session becomes ready and the system says how
    /// far it has taken the stream in; `None` when it knows nothing of
    /// the stream.
    pub fn on_ready(&mut self, cursor: Option<Seq>) -> Resume {
        match cursor {
            Some(cursor) => self.catch_up(cursor),
            None => Resume::Send { from: self.oldest },
        }
    }

    /// What to do when the system says that it expects `expected` next.
    pub fn on_nak(&mut self, expected: Seq) -> Resume {
        self.catch_up(expected - 1)
    }

    /// The system has everything through `through`, and nothing after.
    fn catch_up(&mut self, through: Seq) -> Resume {
        if through >= self.next {
            // The system holds more than this machine remembers having
            // sent: the machine lost its own count. It goes on after
            // what the system has, and says that there may be a hole.
            self.next = through + 1;
            self.acknowledged = through;
            self.oldest = self.next;
            return Resume::Reset {
                from: self.next,
                reason: self.gone.take().unwrap_or(ResetReason::SpoolLost),
            };
        }
        let through = through.max(FIRST_SEQ - 1);
        if through + 1 < self.oldest {
            let reason = self.gone.take().unwrap_or(ResetReason::SpoolLost);
            self.acknowledged = self.acknowledged.max(through);
            return Resume::Reset {
                from: self.oldest,
                reason,
            };
        }
        self.on_ack(through);
        Resume::Send { from: through + 1 }
    }
}

/// What the system does with an event that arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arrival {
    /// The next of its stream: apply it.
    Apply,
    /// Taken in before: ignore it.
    Ignore,
    /// Events before it are missing: apply nothing, and ask for the
    /// stream again from the number expected.
    Missing {
        /// The number expected next.
        expected: Seq,
    },
}

/// What the system answers after a frame of events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Answer {
    /// Everything through this number is taken in.
    Ack(Seq),
    /// Events are missing: send again from this number.
    Nak(Seq),
}

/// What the system does with one frame of events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Which of the frame's events to apply, by their place in it, in
    /// order and in one transaction.
    pub apply: Vec<usize>,
    /// How far the stream is taken in once that transaction commits.
    pub last_seq: Seq,
    /// What to answer after the commit.
    pub answer: Answer,
}

/// One stream as the system that receives it counts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Inbox {
    last_seq: Seq,
}

impl Inbox {
    /// A stream that was taken in through `last_seq`; nothing of a new
    /// stream was.
    #[must_use]
    pub const fn new(last_seq: Seq) -> Self {
        Self { last_seq }
    }

    /// A stream the system has never heard of.
    #[must_use]
    pub const fn unknown() -> Self {
        Self::new(FIRST_SEQ - 1)
    }

    /// The last number taken in.
    #[must_use]
    pub const fn last_seq(&self) -> Seq {
        self.last_seq
    }

    /// What to do with an event numbered `seq`.
    #[must_use]
    pub fn arrival(&self, seq: Seq) -> Arrival {
        match arrival(self.last_seq, seq) {
            revision::Arrival::Next => Arrival::Apply,
            revision::Arrival::Duplicate => Arrival::Ignore,
            revision::Arrival::Gap { from } => Arrival::Missing { expected: from },
        }
    }

    /// What to do with a frame whose events are numbered `numbers`, in
    /// the order the frame has them. Events already taken in are passed
    /// over; the first one that leaves a hole stops the frame, and the
    /// answer then asks for the stream again from the hole. Otherwise
    /// the answer acknowledges how far the stream now stands, also when
    /// nothing in the frame was new, so that a machine whose earlier
    /// acknowledgement was lost learns that it may let go.
    #[must_use]
    pub fn plan(&self, numbers: &[Seq]) -> Plan {
        let mut cursor = *self;
        let mut apply = Vec::new();
        for (place, seq) in numbers.iter().enumerate() {
            match cursor.arrival(*seq) {
                Arrival::Apply => {
                    apply.push(place);
                    cursor.last_seq = *seq;
                }
                Arrival::Ignore => {}
                Arrival::Missing { expected } => {
                    return Plan {
                        apply,
                        last_seq: cursor.last_seq,
                        answer: Answer::Nak(expected),
                    };
                }
            }
        }
        Plan {
            apply,
            last_seq: cursor.last_seq,
            answer: Answer::Ack(cursor.last_seq),
        }
    }

    /// Records that the transaction of `plan` committed.
    pub fn committed(&mut self, plan: &Plan) {
        self.last_seq = plan.last_seq;
    }

    /// Takes a machine's word that the stream goes on from `from`: the
    /// event before it counts as the last one taken in, whatever was.
    pub fn on_reset(&mut self, from: Seq) {
        self.last_seq = from.max(FIRST_SEQ) - 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};

    const A: &str = "0199c1a0-0000-7000-8000-00000000000a";
    const B: &str = "0199c1a0-0000-7000-8000-00000000000b";

    fn round_trip<T: Serialize + DeserializeOwned>(example: Value) -> T {
        let read: T = serde_json::from_value(example.clone()).unwrap();
        assert_eq!(serde_json::to_value(&read).unwrap(), example);
        read
    }

    #[test]
    fn every_stream_is_written_and_read_by_its_name() {
        let (a, b): (Id, Id) = (A.parse().unwrap(), B.parse().unwrap());
        let streams = [
            (Stream::Machine, "machine".to_owned()),
            (Stream::Node, "node".to_owned()),
            (
                Stream::Harness {
                    provider_id: a,
                    agent_id: b,
                },
                format!("harness:{A}:{B}"),
            ),
            (Stream::Provider { provider_id: a }, format!("provider:{A}")),
            (Stream::Env { environment_id: a }, format!("env:{A}")),
            (
                Stream::Session {
                    thread_id: a,
                    segment_id: b,
                },
                format!("session:{A}:{B}"),
            ),
        ];
        for (stream, name) in streams {
            assert_eq!(stream.to_string(), name);
            assert_eq!(name.parse::<Stream>(), Ok(stream));
            assert_eq!(serde_json::to_value(stream).unwrap(), name);
        }
    }

    #[test]
    fn a_name_that_is_no_streams_is_refused() {
        let upper = format!("env:{}", A.to_uppercase());
        let too_long = format!("env:{A}:{B}");
        let too_short = format!("session:{A}");
        for name in [
            "",
            "machine:",
            "machines",
            "env",
            "env:not-an-id",
            &upper,
            &too_long,
            &too_short,
        ] {
            assert!(name.parse::<Stream>().is_err(), "{name}");
        }
    }

    #[test]
    fn each_stream_is_sent_by_the_role_that_knows_what_it_reports() {
        assert_eq!(StreamFamily::Machine.source(), None);
        assert_eq!(StreamFamily::Session.source(), Some(Role::Agent));
        assert_eq!(StreamFamily::Provider.source(), Some(Role::Provider));
        assert_eq!(StreamFamily::Env.source(), Some(Role::Environment));
        assert_eq!(StreamFamily::Node.source(), Some(Role::Node));
    }

    #[test]
    fn stream_frames_read_and_write_as_the_protocol_has_them() {
        let stream = format!("session:{A}:{B}");
        round_trip::<Ev>(json!({
            "s": stream, "q": 41, "at": "2026-10-08T17:04:05.000Z", "k": "session.title", "b": { "title": "Fix login" }
        }));
        let batch: Evb = round_trip(json!({
            "s": stream, "e": [
                { "q": 42, "at": "2026-10-08T17:04:05.100Z", "k": "turn.started", "b": { "turn_id": A, "native_turn_ref": "t1" } },
                { "q": 43, "at": "2026-10-08T17:04:05.200Z", "k": "session.title", "b": { "title": "Fix the login" } }
            ]
        }));
        assert_eq!(batch.events.len(), 2);
        round_trip::<Ack>(json!({ "s": stream, "q": 43 }));
        round_trip::<Nak>(json!({ "s": stream, "expected": 44 }));
        round_trip::<Reset>(json!({ "s": stream, "from": 50, "reason": "spool_overflow" }));
        round_trip::<Eph>(json!({
            "s": format!("env:{A}"), "k": "fs.changed", "at": "2026-10-08T17:04:05.000Z",
            "b": { "watch_id": B, "changed": [], "removed": ["tmp/a.txt"] }
        }));
    }

    #[test]
    fn events_are_numbered_from_one_without_gaps() {
        let mut outbox = Outbox::new();
        assert_eq!(
            [outbox.number(), outbox.number(), outbox.number()],
            [1, 2, 3]
        );
        assert_eq!(outbox.unacknowledged(), 3);
    }

    #[test]
    fn an_acknowledgement_lets_the_machine_delete_what_it_covers() {
        let mut outbox = Outbox::restored(44, 41);
        assert_eq!(outbox.on_ack(42), 42);
        assert_eq!(outbox.oldest(), 43);
        assert_eq!(outbox.unacknowledged(), 1);
        assert_eq!(
            outbox.on_ack(40),
            42,
            "an older acknowledgement takes nothing back"
        );
        assert_eq!(
            outbox.on_ack(99),
            43,
            "nothing is acknowledged that was never sent"
        );
    }

    #[test]
    fn after_a_reconnect_the_machine_sends_again_from_where_the_system_stands() {
        let mut outbox = Outbox::restored(51, 41);
        assert_eq!(outbox.on_ready(Some(43)), Resume::Send { from: 44 });
        assert_eq!(outbox.oldest(), 44, "what the system has is deleted");
    }

    #[test]
    fn a_system_that_has_everything_is_sent_nothing_again() {
        let mut outbox = Outbox::restored(51, 41);
        assert_eq!(outbox.on_ready(Some(50)), Resume::Send { from: 51 });
        assert_eq!(outbox.unacknowledged(), 0);
    }

    #[test]
    fn a_system_that_knows_nothing_of_a_stream_is_sent_everything_the_machine_holds() {
        let mut fresh = Outbox::new();
        fresh.number();
        fresh.number();
        assert_eq!(fresh.on_ready(None), Resume::Send { from: 1 });
        let mut old = Outbox::restored(51, 41);
        assert_eq!(old.on_ready(None), Resume::Send { from: 41 });
    }

    #[test]
    fn a_machine_that_lost_events_the_system_never_saw_says_so_with_a_reset() {
        let mut outbox = Outbox::restored(61, 41);
        assert_eq!(
            outbox.dropped(49, ResetReason::SpoolOverflow),
            Some(Resume::Reset {
                from: 50,
                reason: ResetReason::SpoolOverflow
            })
        );
        assert_eq!(
            outbox.on_ready(Some(43)),
            Resume::Reset {
                from: 50,
                reason: ResetReason::SpoolOverflow
            },
            "the reason is still known when the session comes back"
        );
        assert_eq!(
            outbox.on_ready(Some(49)),
            Resume::Send { from: 50 },
            "once the system stands there, no reset"
        );
    }

    #[test]
    fn dropping_what_was_acknowledged_needs_no_reset() {
        let mut outbox = Outbox::restored(61, 41);
        outbox.on_ack(50);
        assert_eq!(outbox.dropped(45, ResetReason::SpoolOverflow), None);
    }

    #[test]
    fn a_request_to_send_again_from_a_number_the_machine_still_holds_is_served() {
        let mut outbox = Outbox::restored(51, 41);
        assert_eq!(outbox.on_nak(44), Resume::Send { from: 44 });
        assert_eq!(
            outbox.acknowledged(),
            43,
            "the system has everything before what it asks for"
        );
    }

    #[test]
    fn a_request_for_events_the_machine_no_longer_holds_is_answered_with_a_reset() {
        let mut outbox = Outbox::restored(51, 41);
        assert_eq!(
            outbox.on_nak(1),
            Resume::Reset {
                from: 41,
                reason: ResetReason::SpoolLost
            }
        );
    }

    #[test]
    fn a_machine_that_lost_its_own_count_goes_on_after_what_the_system_has() {
        let mut outbox = Outbox::new();
        assert_eq!(
            outbox.on_ready(Some(500)),
            Resume::Reset {
                from: 501,
                reason: ResetReason::SpoolLost
            }
        );
        assert_eq!(outbox.number(), 501);
    }

    #[test]
    fn a_stream_that_is_too_far_ahead_of_its_acknowledgements_stops_sending() {
        let mut outbox = Outbox::new();
        for _ in 0..MAX_UNACKNOWLEDGED_EVENTS - 1 {
            outbox.number();
        }
        assert!(outbox.may_send(0));
        outbox.number();
        assert!(!outbox.may_send(0));
        outbox.on_ack(1);
        assert!(outbox.may_send(0));
        assert!(!outbox.may_send(MAX_UNACKNOWLEDGED_BYTES));
    }

    #[test]
    fn the_system_applies_the_next_event_and_ignores_one_it_has() {
        let inbox = Inbox::new(43);
        assert_eq!(inbox.arrival(44), Arrival::Apply);
        assert_eq!(inbox.arrival(43), Arrival::Ignore);
        assert_eq!(inbox.arrival(12), Arrival::Ignore);
        assert_eq!(Inbox::unknown().arrival(1), Arrival::Apply);
    }

    #[test]
    fn an_event_that_leaves_a_hole_is_not_applied_and_the_stream_is_asked_for_again() {
        let inbox = Inbox::new(43);
        assert_eq!(inbox.arrival(45), Arrival::Missing { expected: 44 });
        let plan = inbox.plan(&[45, 46]);
        assert!(plan.apply.is_empty());
        assert_eq!(plan.answer, Answer::Nak(44));
        assert_eq!(plan.last_seq, 43);
    }

    #[test]
    fn a_batch_sent_again_is_applied_only_where_it_is_new() {
        let mut inbox = Inbox::new(43);
        let plan = inbox.plan(&[42, 43, 44, 45]);
        assert_eq!(plan.apply, [2, 3]);
        assert_eq!(plan.answer, Answer::Ack(45));
        inbox.committed(&plan);
        assert_eq!(inbox.last_seq(), 45);
    }

    #[test]
    fn a_batch_the_system_has_whole_is_acknowledged_again_and_applied_not_at_all() {
        let inbox = Inbox::new(45);
        let plan = inbox.plan(&[44, 45]);
        assert!(plan.apply.is_empty());
        assert_eq!(plan.answer, Answer::Ack(45));
    }

    #[test]
    fn a_batch_with_a_hole_in_it_is_applied_up_to_the_hole() {
        let inbox = Inbox::new(43);
        let plan = inbox.plan(&[44, 46, 47]);
        assert_eq!(plan.apply, [0]);
        assert_eq!(plan.last_seq, 44);
        assert_eq!(plan.answer, Answer::Nak(45));
    }

    #[test]
    fn events_that_arrive_dropped_doubled_and_out_of_order_are_each_applied_once_in_order() {
        // The machine holds 1..=6; the link loses, repeats and reorders frames.
        let mut outbox = Outbox::new();
        for _ in 0..6 {
            outbox.number();
        }
        let mut inbox = Inbox::unknown();
        let mut applied = Vec::new();
        let mut frames: Vec<Vec<Seq>> = vec![vec![1, 2], vec![1, 2], vec![4], vec![3], vec![5, 6]];
        while !frames.is_empty() {
            let frame = frames.remove(0);
            let plan = inbox.plan(&frame);
            applied.extend(plan.apply.iter().map(|place| frame[*place]));
            inbox.committed(&plan);
            match plan.answer {
                Answer::Ack(through) => {
                    outbox.on_ack(through);
                }
                Answer::Nak(expected) => {
                    if let Resume::Send { from } = outbox.on_nak(expected) {
                        frames.insert(0, (from..outbox.next()).collect());
                    }
                }
            }
        }
        assert_eq!(applied, [1, 2, 3, 4, 5, 6]);
        assert_eq!(outbox.unacknowledged(), 0);
    }

    #[test]
    fn a_reset_moves_the_systems_place_to_just_before_where_the_stream_goes_on() {
        let mut inbox = Inbox::new(43);
        inbox.on_reset(50);
        assert_eq!(inbox.last_seq(), 49);
        assert_eq!(inbox.arrival(50), Arrival::Apply);
        let session: Stream = format!("session:{A}:{B}").parse().unwrap();
        assert!(session.resyncs_after_reset());
        assert!(!Stream::Machine.resyncs_after_reset());
    }

    #[test]
    fn diagnostics_are_known_by_how_their_kind_begins() {
        assert!(is_diagnostic("raw.frame"));
        assert!(!is_diagnostic("item.delta"));
        assert_eq!(SpoolRelief::ORDER[0], SpoolRelief::Diagnostics);
        assert_eq!(SpoolRelief::ORDER[2], SpoolRelief::OldestEvents);
    }
}
