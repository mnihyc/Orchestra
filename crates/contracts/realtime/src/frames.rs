//! Authentication, subscription, sync, delta and event frames.
//!
//! One WebSocket per browser tab carries every live update. Each frame is
//! a text frame holding one JSON object whose member `op` says what it
//! is. [`ClientFrame`] is what the browser sends and [`ServerFrame`] what
//! the server sends; each variant holds a struct named after the `op`.
//!
//! The socket never accepts a mutation: HTTP loads and changes things,
//! and the socket tells which rows changed. The data of `sync` and
//! `event` frames depends on the feed they are of, so the frames hold it
//! as the JSON object it is, and [`SyncFrame::data`] and
//! [`EventFrame::data`] read it as the type that feed defines
//! ([`feeds`](crate::feeds)).
//!
//! The feed in a frame the client sends is kept as the text it was written
//! as, because a client that names no feed gets an `error` frame with the
//! code `bad_feed` and the connection stays open; every feed in a frame
//! the server sends is one it has made, and is read.
//!
//! Only text frames are used on this socket. A binary frame, a text that
//! is not one JSON object with an `op`, a frame over
//! [`MAX_CLIENT_FRAME_BYTES`] from the client, and an `op` or a member
//! that the protocol does not have are each malformed, and the receiving
//! side closes the connection with [`CloseCode::Malformed`].
//!
//! A valid same-person `reauth` renews the connection without a reply and keeps its subscriptions.
//! Reusing a subscription id replaces it and answers `subscribed`, without closing the new id.
//! `unsub` answers `closed` with `unsubscribed` once; an unknown id answers `error` with `not_found`. A malformed feed
//! name answers `error` with `bad_feed` and keeps the socket open. A `visible` or `typing` frame
//! needs an open subscription to act; presence merges visible tabs across API processes by person.
//! A sequence gap beyond a feed's limit, or a sync frame beyond its queue budget, answers `resync`;
//! the client repairs through HTTP and subscribes again from the sequence that repair returned.

use crate::delta::DeltaField;
use crate::feeds::{BadFeed, DataError, EventData, Feed, SyncData};
use crate::presence::PresenceUser;
use crate::sync::SubscribeMode;
use crate::version::{CloseCode, MAX_CLIENT_FRAME_BYTES};
use orch_types::{Id, Json, Seq, Time};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Map;
use std::fmt;

/// The number by which a client names one of its subscriptions, chosen by
/// the client and unique among those of its connection. Every frame about
/// a subscription carries it.
pub type SubId = i64;

/// The data of a `sync` or an `event` frame: a JSON object.
pub type DataObject = Map<String, Json>;

// ------------------------------------------------------------------ the client

/// A frame the browser sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ClientFrame {
    /// The first frame of a connection: who is speaking.
    Auth(AuthFrame),
    /// A new access token for a connection that is open.
    Reauth(ReauthFrame),
    /// A request to follow a feed.
    Sub(SubFrame),
    /// The end of a subscription.
    Unsub(UnsubFrame),
    /// Whether a feed's view is in front of the person.
    Visible(VisibleFrame),
    /// The person is writing in a chat or a document.
    Typing(TypingFrame),
    /// A question whether the line is alive, and how long it takes.
    Ping(PingFrame),
}

/// Says who is speaking. It has to be the first frame, within
/// [`AUTH_TIMEOUT_SECS`](crate::version::AUTH_TIMEOUT_SECS) of the
/// connection opening.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct AuthFrame {
    /// The access token.
    pub token: String,
}

impl fmt::Debug for AuthFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthFrame")
            .field("token", &"<redacted>")
            .finish()
    }
}

/// Gives the connection a new access token. A token lasts a short while,
/// and the client sends this before the `access_expires_at` it was told;
/// a connection that outlives its token is closed.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct ReauthFrame {
    /// The new access token.
    pub token: String,
}

impl fmt::Debug for ReauthFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReauthFrame")
            .field("token", &"<redacted>")
            .finish()
    }
}

/// Asks to follow a feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct SubFrame {
    /// Exact native segment for a raw subscription; required there, forbidden on other feeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_segment_id: Option<Id>,
    /// A thread history to observe without changing execution; omission follows active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_id: Option<Id>,
    /// Scope of the caller's thread sequence; changed scope requires HTTP resync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_revision: Option<i64>,
    /// A screen's opening UUID, shared with its x-orch-opening HTTP header. It deduplicates
    /// private-opening audit rows; it grants no access and may be left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opening_id: Option<Id>,
    /// The number by which the client will name this subscription.
    pub id: SubId,
    /// The feed's name: `thread:<id>`, `topic:<id>`, `org:<id>`, `user`,
    /// `chat:<id>`, `doc:<id>` or `env:<id>` for a feed with a sequence;
    /// `thread:<id>:raw`, `env:<id>:stats`, `env:<id>:procs`,
    /// `env:<id>:dir:<path>` or `resource:<kind>:<id>` for one without (see
    /// [`Feed`]).
    pub feed: String,
    /// For a durable feed, the highest sequence the client holds. The
    /// server sends every row stamped after it. Left out, the stream
    /// begins at the feed's sequence now. Ignored by a feed without a
    /// sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<Seq>,
    /// For a thread's feed: also send the growing text of items that sub-agents
    /// produce, which otherwise arrive as rows only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_streams: Option<bool>,
}

impl SubFrame {
    /// The feed asked for; a name that is no feed's is answered with an
    /// `error` frame whose code is `bad_feed`.
    pub fn feed(&self) -> Result<Feed, BadFeed> {
        self.feed.parse()
    }
}

/// Ends a subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct UnsubFrame {
    /// The subscription's number.
    pub id: SubId,
}

/// Says whether the view of a feed is in front of the person: the page is
/// visible and the route shows it. It counts for a feed the connection is
/// subscribed to that says who is looking, and is ignored for any other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct VisibleFrame {
    /// The feed's name, as in `sub`.
    pub feed: String,
    /// Whether it is in front of them.
    pub visible: bool,
}

impl VisibleFrame {
    /// The feed named.
    pub fn feed(&self) -> Result<Feed, BadFeed> {
        self.feed.parse()
    }
}

/// Says that the person is writing in a chat or a document. Sent at most
/// once every three seconds, and only for a feed the connection is
/// subscribed to that has typing; the server ignores it otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct TypingFrame {
    /// The feed's name, as in `sub`: a chat's or a document's.
    pub feed: String,
    /// On a document's feed, the block their caret is in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Id>,
}

impl TypingFrame {
    /// The feed named.
    pub fn feed(&self) -> Result<Feed, BadFeed> {
        self.feed.parse()
    }
}

/// A question whether the line is alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(inline)]
pub struct PingFrame {
    /// A number the answer repeats, from which the client measures the
    /// round trip.
    pub n: i64,
}

impl ClientFrame {
    /// Every `op` a client may send.
    pub const OPS: [&'static str; 7] = [
        "auth", "reauth", "sub", "unsub", "visible", "typing", "ping",
    ];

    /// The frame's `op`.
    #[must_use]
    pub const fn op(&self) -> &'static str {
        match self {
            Self::Auth(_) => "auth",
            Self::Reauth(_) => "reauth",
            Self::Sub(_) => "sub",
            Self::Unsub(_) => "unsub",
            Self::Visible(_) => "visible",
            Self::Typing(_) => "typing",
            Self::Ping(_) => "ping",
        }
    }

    /// Reads a text frame from a client.
    ///
    /// A text of more than [`MAX_CLIENT_FRAME_BYTES`] bytes, a text that is
    /// not a JSON object with an `op`, an `op` the protocol does not have,
    /// and members that are missing, wrong or not in the protocol are each
    /// a [`FrameFault`], and the connection is closed with the code of
    /// [`FrameFault::close_code`].
    pub fn decode(text: &str) -> Result<Self, FrameFault> {
        decode_text(text, &Self::OPS, Some(MAX_CLIENT_FRAME_BYTES))
    }

    /// Writes the frame as the text frame it is.
    pub fn encode(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// The `auth` frame a connection has to begin with. Any other frame
    /// first means that authentication is missing, and the connection is
    /// closed with the code of [`CloseCode::Unauthenticated`].
    pub fn first(&self) -> Result<&AuthFrame, CloseCode> {
        match self {
            Self::Auth(auth) => Ok(auth),
            _ => Err(CloseCode::Unauthenticated),
        }
    }
}

// ------------------------------------------------------------------ the server

/// A frame the server sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ServerFrame {
    /// The word that the connection stands, with what the client needs to
    /// know of it.
    Ready(ReadyFrame),
    /// The answer to a subscription.
    Subscribed(SubscribedFrame),
    /// Rows of a durable feed that changed.
    Sync(SyncFrame),
    /// Growing text of an item of a thread.
    Delta(DeltaFrame),
    /// Who is looking.
    Presence(PresenceFrame),
    /// Something that happened on a feed and is not a row.
    Event(EventFrame),
    /// The word that frames were dropped and the client reads again.
    Resync(ResyncFrame),
    /// The end of a subscription.
    Closed(ClosedFrame),
    /// A request that could not be served.
    Error(ErrorFrame),
    /// The answer to a ping.
    Pong(PongFrame),
}

/// The word that the connection stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct ReadyFrame {
    /// The connection's name, for logs and support.
    pub connection_id: String,
    /// The signed-in user.
    pub user_id: Id,
    /// The server's time.
    pub server_time: Time,
    /// When the token the connection was given expires; the client sends
    /// `reauth` before.
    pub access_expires_at: Time,
    /// How often the client pings, in seconds.
    pub heartbeat_secs: u32,
    /// The build of the server. When it differs from the build the page
    /// was loaded with, the client says that a new version is available
    /// and never reloads by itself while a draft is open.
    pub build: String,
}

/// The raw-number floor of one newly admitted segment subscription, never a durable WS cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RawSubscription {
    /// The exact native segment requested by this subscription.
    pub segment_id: Id,
    /// Only subsequently committed segment numbers above this floor can arrive live.
    pub after: i64,
}

/// The answer to a subscription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct SubscribedFrame {
    /// Admitted raw segment and live-only floor; present only on a raw subscription.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<RawSubscription>,
    /// The actual read scope for a conversation or raw subscription; absent on other feeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<orch_api_contract::threads::ThreadContinuationProjection>,
    /// The subscription's number.
    pub id: SubId,
    /// The feed.
    pub feed: Feed,
    /// Where the stream of a durable feed begins; `null` for a feed without
    /// a sequence.
    pub seq: Option<Seq>,
    /// How the subscription begins.
    pub mode: SubscribeMode,
}

/// Rows of a durable feed that changed: every row stamped in `(prev, seq]`
/// that the subscriber may see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct SyncFrame {
    /// The subscription's number.
    pub id: SubId,
    /// The feed.
    pub feed: Feed,
    /// The `seq` of the frame before this one on the subscription, or
    /// where the stream began.
    pub prev: Seq,
    /// The highest stamp among the rows it carries.
    pub seq: Seq,
    /// The rows, as members named after their kind, each there only when
    /// rows of its kind changed. A thread's feed has `thread`, `items`,
    /// `turns`, `inputs` and `segments` (`ThreadData`); a topic's
    /// `TopicData`, an environment's `EnvData`, an organization's
    /// `OrgData`, the caller's own `UserData`, a chat's `ChatData` and a
    /// document's `DocData`. A row replaces the stored copy when its
    /// `seq_updated` is higher. [`SyncData`] reads it by the feed.
    pub data: DataObject,
}

impl SyncFrame {
    /// A frame of `feed` carrying `data`.
    pub fn new(
        id: SubId,
        feed: Feed,
        prev: Seq,
        seq: Seq,
        data: &SyncData,
    ) -> Result<Self, DataError> {
        Ok(Self {
            id,
            feed,
            prev,
            seq,
            data: data.to_object()?,
        })
    }

    /// The data, read as the feed of the frame defines it.
    pub fn data(&self) -> Result<SyncData, DataError> {
        SyncData::decode(&self.feed, Json::Object(self.data.clone()))
    }
}

/// A piece of the text an item is growing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct DeltaFrame {
    /// The subscription's number.
    pub id: SubId,
    /// The thread's feed.
    pub feed: Feed,
    /// The item that grows.
    pub item_id: Id,
    /// The field of the item that grows.
    pub field: DeltaField,
    /// Where in the field this piece begins, in the field's unit.
    pub offset: i64,
    /// How long this piece is, in the same unit.
    pub len: i64,
    /// The piece. For output it is the bytes decoded as UTF-8 with invalid
    /// sequences replaced, so that its length need not be `len`.
    pub data: String,
}

/// Who is looking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct PresenceFrame {
    /// The subscription's number.
    pub id: SubId,
    /// The feed.
    pub feed: Feed,
    /// Everybody who is looking now, the longest first.
    pub users: Vec<PresenceUser>,
}

/// Something that happened on a feed and is not a row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct EventFrame {
    /// The subscription's number.
    pub id: SubId,
    /// The feed.
    pub feed: Feed,
    /// What happened, by the feed. On the caller's own feed a `what` among
    /// `read_state`, `subscription`, `favorites`, `draft`, `chat_state`,
    /// `chat_draft`, `chat`, `approvals`, `machines`, `resources`, `route`
    /// and `state` (`UserEvent`); on a chat's or a document's feed
    /// `{ what: "typing", user, at? }` (`TypingNotice`); on a resource's
    /// feed `changed`, `login` or `job` (`ResourceEvent`); on the feeds
    /// that watch an environment what they see, or `{ offline: true }`
    /// once; on a thread's raw feed a source-bound `RawData`. [`EventData`] reads it
    /// by the feed.
    pub data: DataObject,
}

impl EventFrame {
    /// A frame of `feed` carrying `data`.
    pub fn new(id: SubId, feed: Feed, data: &EventData) -> Result<Self, DataError> {
        Ok(Self {
            id,
            feed,
            data: data.to_object()?,
        })
    }

    /// The data, read as the feed of the frame defines it.
    pub fn data(&self) -> Result<EventData, DataError> {
        EventData::decode(&self.feed, Json::Object(self.data.clone()))
    }
}

/// The word that what was waiting for the client was dropped, and the
/// client reads the feed again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct ResyncFrame {
    /// The subscription's number.
    pub id: SubId,
    /// The feed.
    pub feed: Feed,
    /// True retires the previous authority/read projection and requires a full
    /// HTTP baseline followed by a new subscription. Absent keeps gap repair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reload: Option<bool>,
}

/// Why a subscription ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FeedClosedReason {
    /// The person no longer holds the permission the feed needs.
    Revoked,
    /// What the feed was about was deleted.
    Deleted,
    /// The client asked for it to end with `unsub`, which the server
    /// answers with this.
    Unsubscribed,
}

/// The end of a subscription. Nothing more is sent on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct ClosedFrame {
    /// The subscription's number.
    pub id: SubId,
    /// The feed.
    pub feed: Feed,
    /// Why.
    pub reason: FeedClosedReason,
}

/// What a request could not be served for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SocketErrorCode {
    /// The person may not follow that feed.
    Forbidden,
    /// What the feed is about does not exist, or is not visible to them.
    NotFound,
    /// The connection holds as many subscriptions as it may.
    TooManySubscriptions,
    /// The name is no feed's.
    BadFeed,
    /// This read would expose an opaque native session outside its complete original history.
    ReadUnavailable,
}

/// A request that could not be served. The connection stays open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct ErrorFrame {
    /// The subscription it is about, when it is about one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<SubId>,
    /// What went wrong.
    pub code: SocketErrorCode,
    /// In words, for a person who is looking for the cause.
    pub detail: String,
}

/// The answer to a ping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[schemars(inline)]
pub struct PongFrame {
    /// The number the ping carried.
    pub n: i64,
}

impl ServerFrame {
    /// Every `op` the server sends.
    pub const OPS: [&'static str; 10] = [
        "ready",
        "subscribed",
        "sync",
        "delta",
        "presence",
        "event",
        "resync",
        "closed",
        "error",
        "pong",
    ];

    /// The frame's `op`.
    #[must_use]
    pub const fn op(&self) -> &'static str {
        match self {
            Self::Ready(_) => "ready",
            Self::Subscribed(_) => "subscribed",
            Self::Sync(_) => "sync",
            Self::Delta(_) => "delta",
            Self::Presence(_) => "presence",
            Self::Event(_) => "event",
            Self::Resync(_) => "resync",
            Self::Closed(_) => "closed",
            Self::Error(_) => "error",
            Self::Pong(_) => "pong",
        }
    }

    /// The subscription the frame is about; none for the frames about the
    /// connection, and for an error that is about no subscription.
    #[must_use]
    pub const fn subscription(&self) -> Option<SubId> {
        match self {
            Self::Subscribed(frame) => Some(frame.id),
            Self::Sync(frame) => Some(frame.id),
            Self::Delta(frame) => Some(frame.id),
            Self::Presence(frame) => Some(frame.id),
            Self::Event(frame) => Some(frame.id),
            Self::Resync(frame) => Some(frame.id),
            Self::Closed(frame) => Some(frame.id),
            Self::Error(frame) => frame.id,
            Self::Ready(_) | Self::Pong(_) => None,
        }
    }

    /// Reads a text frame from the server.
    pub fn decode(text: &str) -> Result<Self, FrameFault> {
        decode_text(text, &Self::OPS, None)
    }

    /// Writes the frame as the text frame it is.
    pub fn encode(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

// ------------------------------------------------------------------ reading frames

/// Why a text is not a frame of the protocol. Every one is a reason for the
/// receiving side to close the connection as malformed.
#[derive(Debug, thiserror::Error)]
pub enum FrameFault {
    /// The text is longer than a client may send.
    #[error("the frame is {size} bytes long and a client may send {max}")]
    TooLarge {
        /// Its length.
        size: usize,
        /// The most allowed.
        max: usize,
    },
    /// The text is not JSON.
    #[error("the frame is not JSON: {0}")]
    NotJson(#[source] serde_json::Error),
    /// The text is JSON, but not an object with an `op` that is a string.
    #[error("the frame is not an object with an `op`")]
    NoOp,
    /// The `op` is not one of the protocol's.
    #[error("`{0}` is not an operation of the protocol")]
    UnknownOp(String),
    /// The `op` is known, but the members are missing, of the wrong type or
    /// not the frame's.
    #[error("the members of a `{op}` frame are wrong: {source}")]
    Members {
        /// The frame's `op`.
        op: String,
        /// What is wrong.
        #[source]
        source: serde_json::Error,
    },
}

impl FrameFault {
    /// The code a connection is closed with for this fault.
    #[must_use]
    pub const fn close_code(&self) -> CloseCode {
        CloseCode::Malformed
    }
}

/// Reads one text frame of a family whose operations are `ops`, telling
/// apart what is wrong with it.
pub(crate) fn decode_text<T: DeserializeOwned>(
    text: &str,
    ops: &[&str],
    max_bytes: Option<usize>,
) -> Result<T, FrameFault> {
    if let Some(max) = max_bytes
        && text.len() > max
    {
        return Err(FrameFault::TooLarge {
            size: text.len(),
            max,
        });
    }
    let value: Json = serde_json::from_str(text).map_err(FrameFault::NotJson)?;
    let op = value
        .get("op")
        .and_then(Json::as_str)
        .ok_or(FrameFault::NoOp)?;
    if !ops.contains(&op) {
        return Err(FrameFault::UnknownOp(op.to_owned()));
    }
    let op = op.to_owned();
    serde_json::from_value(value).map_err(|source| FrameFault::Members { op, source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::{Applied, DeltaAction};
    use crate::feeds::fixtures::{A, AT, B, agent_message, id, process, thread, user};
    use crate::feeds::{ThreadData, UserEvent};
    use serde_json::json;

    const ITEM: &str = "0199c1a0-0000-7000-8000-0000000000a9";

    fn feed() -> String {
        format!("thread:{A}")
    }

    /// Reads a frame the client sent, and checks that it is written as it was.
    fn client(written: &Json) -> ClientFrame {
        let frame = ClientFrame::decode(&written.to_string()).unwrap();
        assert_eq!(&serde_json::to_value(&frame).unwrap(), written);
        assert_eq!(frame.op(), written["op"]);
        frame
    }

    /// Reads a frame the server sent, and checks that it is written as it was.
    fn server(written: &Json) -> ServerFrame {
        let frame = ServerFrame::decode(&written.to_string()).unwrap();
        assert_eq!(&serde_json::to_value(&frame).unwrap(), written);
        assert_eq!(frame.op(), written["op"]);
        frame
    }

    // ------------------------------------------------------------ the example session

    #[test]
    fn authority_resync_is_explicit_without_changing_legacy_gap_repair() {
        let feed = format!("org:{A}");
        let ServerFrame::Resync(gap) = server(&json!({"op":"resync","id":1,"feed":feed})) else {
            panic!("expected a resync frame");
        };
        assert_eq!(gap.reload, None);
        let ServerFrame::Resync(authority) =
            server(&json!({"op":"resync","id":1,"feed":feed,"reload":true}))
        else {
            panic!("expected a resync frame");
        };
        assert_eq!(authority.reload, Some(true));
        assert!(
            ServerFrame::decode(
                &json!({"op":"resync","id":1,"feed":feed,"reload":"true"}).to_string()
            )
            .is_err()
        );
    }

    #[test]
    fn the_example_session_of_the_protocol_reads_and_writes_frame_by_frame() {
        let feed = feed();
        let auth = json!({ "op": "auth", "token": "oat_9f8e7d" });
        let ready = json!({
            "op": "ready", "connection_id": "c7", "user_id": B, "server_time": AT,
            "access_expires_at": "2026-10-08T17:19:05.123Z", "heartbeat_secs": 25, "build": "0.1.0+412"
        });
        let sub = json!({ "op": "sub", "id": 1, "feed": feed, "since": 412 });
        let subscribed =
            json!({ "op": "subscribed", "id": 1, "feed": feed, "seq": 412, "mode": "delta" });
        let presence = json!({ "op": "presence", "id": 1, "feed": feed, "users": [ { "user": user(), "since": AT } ] });
        let first_sync = json!({
            "op": "sync", "id": 1, "feed": feed, "prev": 412, "seq": 414,
            "data": { "thread": thread(), "turns": [], "items": [agent_message("in_progress", "")] }
        });
        let first_delta = json!({
            "op": "delta", "id": 1, "feed": feed, "item_id": ITEM, "field": "text",
            "offset": 0, "len": 18, "data": "I'll start by read"
        });
        let second_delta = json!({
            "op": "delta", "id": 1, "feed": feed, "item_id": ITEM, "field": "text",
            "offset": 18, "len": 16, "data": "ing the handler."
        });
        let last_sync = json!({
            "op": "sync", "id": 1, "feed": feed, "prev": 414, "seq": 415,
            "data": { "items": [agent_message("completed", "I'll start by reading the handler.")] }
        });

        assert!(matches!(client(&auth), ClientFrame::Auth(_)));
        assert!(matches!(server(&ready), ServerFrame::Ready(_)));
        let ClientFrame::Sub(sub) = client(&sub) else {
            panic!("not a sub")
        };
        assert_eq!((sub.id, sub.since, sub.sub_streams), (1, Some(412), None));
        assert_eq!(sub.feed(), Ok(Feed::Thread(id())));
        let ServerFrame::Subscribed(subscribed) = server(&subscribed) else {
            panic!("not subscribed")
        };
        assert_eq!(
            (subscribed.seq, subscribed.mode),
            (Some(412), SubscribeMode::Delta)
        );
        let ServerFrame::Presence(presence) = server(&presence) else {
            panic!("not presence")
        };
        assert_eq!(presence.users[0].user.username, "bo");

        let ServerFrame::Sync(first) = server(&first_sync) else {
            panic!("not a sync")
        };
        let SyncData::Thread(ThreadData {
            thread: Some(row),
            items: Some(items),
            ..
        }) = first.data().unwrap()
        else {
            panic!("not the data of a thread")
        };
        assert_eq!(
            (first.prev, first.seq, row.seq, items.len()),
            (412, 414, 415, 1)
        );

        // The two deltas, taken in turn from the length the first sync's row has, make the text of the last.
        let mut applied = Applied::from_row(0);
        let mut text = String::new();
        for written in [&first_delta, &second_delta] {
            let ServerFrame::Delta(delta) = server(written) else {
                panic!("not a delta")
            };
            assert_eq!(delta.field.len_of(&delta.data), Some(delta.len));
            assert_eq!(
                applied.on_delta(delta.offset, delta.len),
                DeltaAction::Append
            );
            text.push_str(&delta.data);
        }
        let ServerFrame::Sync(last) = server(&last_sync) else {
            panic!("not a sync")
        };
        let SyncData::Thread(ThreadData {
            items: Some(items), ..
        }) = last.data().unwrap()
        else {
            panic!("not items")
        };
        assert_eq!((last.prev, last.seq), (first.seq, 415));
        assert_eq!(
            serde_json::to_value(&items[0]).unwrap()["payload"]["text"],
            text
        );
        assert_eq!(
            serde_json::to_value(&items[0]).unwrap()["payload"]["text_len"],
            applied.get()
        );
    }

    // ------------------------------------------------------------ the client's frames

    #[test]
    fn every_frame_a_client_may_send_reads_and_writes_as_the_protocol_has_it() {
        let feed = feed();
        let frames = [
            json!({ "op": "auth", "token": "oat_9f8e7d" }),
            json!({ "op": "reauth", "token": "oat_1a2b3c" }),
            json!({ "op": "sub", "id": 1, "feed": feed }),
            json!({ "op": "sub", "id": 2, "feed": feed, "since": 0, "sub_streams": true }),
            json!({ "op": "sub", "id": 3, "feed": "user", "since": 17 }),
            json!({ "op": "unsub", "id": 2 }),
            json!({ "op": "visible", "feed": feed, "visible": true }),
            json!({ "op": "visible", "feed": feed, "visible": false }),
            json!({ "op": "typing", "feed": format!("chat:{A}") }),
            json!({ "op": "typing", "feed": format!("doc:{A}"), "at": B }),
            json!({ "op": "ping", "n": 7 }),
            json!({ "op": "ping", "n": 1_791_681_824_881_i64 }),
        ];
        for written in &frames {
            client(written);
        }
        let ops: Vec<&str> = frames
            .iter()
            .map(|written| written["op"].as_str().unwrap())
            .collect();
        for op in ClientFrame::OPS {
            assert!(ops.contains(&op), "no example of {op}");
        }
    }

    #[test]
    fn a_member_that_may_be_left_out_is_not_written_when_it_is() {
        let sub = ClientFrame::Sub(SubFrame {
            raw_segment_id: None,
            continuation_id: None,
            projection_revision: None,
            opening_id: None,
            id: 1,
            feed: feed(),
            since: None,
            sub_streams: None,
        });
        assert_eq!(
            serde_json::to_value(sub).unwrap(),
            json!({ "op": "sub", "id": 1, "feed": feed() })
        );
        let typing = ClientFrame::Typing(TypingFrame {
            feed: format!("chat:{A}"),
            at: None,
        });
        assert_eq!(
            serde_json::to_value(typing).unwrap(),
            json!({ "op": "typing", "feed": format!("chat:{A}") })
        );
    }

    #[test]
    fn a_frame_with_a_member_the_protocol_does_not_have_is_malformed() {
        for text in [
            r#"{"op":"ping","n":1,"extra":true}"#,
            r#"{"op":"auth","token":"t","scope":"all"}"#,
            r#"{"op":"sub","id":1,"feed":"user","from":3}"#,
        ] {
            let fault = ClientFrame::decode(text).unwrap_err();
            assert!(matches!(fault, FrameFault::Members { .. }), "{text}");
            assert_eq!(fault.close_code(), CloseCode::Malformed);
        }
    }

    #[test]
    fn a_text_that_is_not_a_frame_of_the_protocol_closes_the_connection_as_malformed() {
        type Is = fn(&FrameFault) -> bool;
        let cases: [(&str, Is); 9] = [
            ("not json at all", |fault| {
                matches!(fault, FrameFault::NotJson(_))
            }),
            ("", |fault| matches!(fault, FrameFault::NotJson(_))),
            ("[1,2]", |fault| matches!(fault, FrameFault::NoOp)),
            ("\"ping\"", |fault| matches!(fault, FrameFault::NoOp)),
            (r#"{"n":1}"#, |fault| matches!(fault, FrameFault::NoOp)),
            (r#"{"op":7}"#, |fault| matches!(fault, FrameFault::NoOp)),
            (
                r#"{"op":"teleport"}"#,
                |fault| matches!(fault, FrameFault::UnknownOp(op) if op == "teleport"),
            ),
            // A frame of the server's is not one a client may send.
            (r#"{"op":"pong","n":1}"#, |fault| {
                matches!(fault, FrameFault::UnknownOp(_))
            }),
            (
                r#"{"op":"sub","id":"one","feed":"user"}"#,
                |fault| matches!(fault, FrameFault::Members { op, .. } if op == "sub"),
            ),
        ];
        for (text, expected) in cases {
            let fault = ClientFrame::decode(text).unwrap_err();
            assert!(expected(&fault), "{text}: {fault}");
            assert_eq!(fault.close_code().code(), 4400);
        }
    }

    #[test]
    fn a_frame_may_be_sixteen_kibibytes_and_not_a_byte_more() {
        let frame = r#"{"op":"ping","n":1}"#;
        let exactly = format!(
            "{frame}{}",
            " ".repeat(MAX_CLIENT_FRAME_BYTES - frame.len())
        );
        assert_eq!(exactly.len(), MAX_CLIENT_FRAME_BYTES);
        assert!(ClientFrame::decode(&exactly).is_ok());
        let over = format!("{exactly} ");
        let fault = ClientFrame::decode(&over).unwrap_err();
        assert!(matches!(fault, FrameFault::TooLarge { size, max } if size == max + 1));
        assert_eq!(fault.close_code(), CloseCode::Malformed);
        // A server's frames are not held to what a client may send.
        let long = json!({ "op": "delta", "id": 1, "feed": feed(), "item_id": ITEM, "field": "output",
                           "offset": 0, "len": 40_000, "data": "x".repeat(40_000) });
        assert!(ServerFrame::decode(&long.to_string()).is_ok());
    }

    #[test]
    fn a_name_that_is_no_feed_is_still_a_frame_so_that_the_server_can_answer_bad_feed() {
        let ClientFrame::Sub(sub) =
            ClientFrame::decode(r#"{"op":"sub","id":4,"feed":"weather:today"}"#).unwrap()
        else {
            panic!("not a sub")
        };
        let fault = sub.feed().unwrap_err();
        let answer = ErrorFrame {
            id: Some(sub.id),
            code: SocketErrorCode::BadFeed,
            detail: fault.to_string(),
        };
        assert_eq!(
            serde_json::to_value(ServerFrame::Error(answer)).unwrap(),
            json!({ "op": "error", "id": 4, "code": "bad_feed", "detail": "`weather:today` is not the name of a feed" })
        );
        let visible = VisibleFrame {
            feed: "weather:today".to_owned(),
            visible: true,
        };
        assert!(visible.feed().is_err());
        assert_eq!(
            TypingFrame {
                feed: format!("chat:{A}"),
                at: None
            }
            .feed(),
            Ok(Feed::Chat(id()))
        );
    }

    #[test]
    fn a_connection_begins_with_auth_and_with_nothing_else() {
        let auth = ClientFrame::decode(r#"{"op":"auth","token":"oat_9f8e7d"}"#).unwrap();
        assert_eq!(
            auth.first().map(|frame| frame.token.as_str()),
            Ok("oat_9f8e7d")
        );
        for text in [
            r#"{"op":"ping","n":1}"#,
            r#"{"op":"sub","id":1,"feed":"user"}"#,
            r#"{"op":"reauth","token":"t"}"#,
        ] {
            let first = ClientFrame::decode(text).unwrap();
            assert_eq!(first.first(), Err(CloseCode::Unauthenticated), "{text}");
        }
    }

    #[test]
    fn a_token_is_never_printed() {
        let auth = ClientFrame::Auth(AuthFrame {
            token: "oat_9f8e7d".to_owned(),
        });
        let reauth = ClientFrame::Reauth(ReauthFrame {
            token: "oat_1a2b3c".to_owned(),
        });
        let printed = format!("{auth:?} {reauth:?}");
        assert!(!printed.contains("oat_"), "{printed}");
        assert!(printed.contains("<redacted>"));
    }

    // ------------------------------------------------------------ the server's frames

    #[test]
    fn every_frame_the_server_sends_reads_and_writes_as_the_protocol_has_it() {
        let feed = feed();
        let frames = [
            json!({ "op": "ready", "connection_id": "c7", "user_id": B, "server_time": AT,
                    "access_expires_at": AT, "heartbeat_secs": 25, "build": "0.1.0+412" }),
            json!({ "op": "subscribed", "id": 1, "feed": feed, "seq": 412, "mode": "delta" }),
            json!({ "op": "subscribed", "id": 1, "feed": feed, "seq": 900, "mode": "reload" }),
            json!({ "op": "subscribed", "id": 2, "feed": format!("env:{A}:stats"), "seq": null, "mode": "ephemeral" }),
            json!({ "op": "sync", "id": 1, "feed": feed, "prev": 412, "seq": 414, "data": { "items": [] } }),
            json!({ "op": "delta", "id": 1, "feed": feed, "item_id": ITEM, "field": "text", "offset": 0, "len": 3, "data": "abc" }),
            json!({ "op": "delta", "id": 1, "feed": feed, "item_id": ITEM, "field": "summary", "offset": 3, "len": 1, "data": "d" }),
            json!({ "op": "delta", "id": 1, "feed": feed, "item_id": ITEM, "field": "output", "offset": 100, "len": 2, "data": "\u{fffd}" }),
            json!({ "op": "presence", "id": 1, "feed": feed, "users": [] }),
            json!({ "op": "presence", "id": 1, "feed": feed, "users": [ { "user": user(), "since": AT } ] }),
            json!({ "op": "event", "id": 3, "feed": format!("chat:{A}"), "data": { "what": "typing", "user": user() } }),
            json!({ "op": "resync", "id": 1, "feed": feed }),
            json!({ "op": "resync", "id": 1, "feed": feed, "reload": true }),
            json!({ "op": "closed", "id": 1, "feed": feed, "reason": "revoked" }),
            json!({ "op": "closed", "id": 1, "feed": feed, "reason": "deleted" }),
            json!({ "op": "closed", "id": 1, "feed": feed, "reason": "unsubscribed" }),
            json!({ "op": "error", "id": 1, "code": "forbidden", "detail": "no thread.view" }),
            json!({ "op": "error", "id": 1, "code": "not_found", "detail": "no such thread" }),
            json!({ "op": "error", "id": 65, "code": "too_many_subscriptions", "detail": "64 at most" }),
            json!({ "op": "error", "code": "bad_feed", "detail": "no such feed" }),
            json!({ "op": "pong", "n": 7 }),
        ];
        for written in &frames {
            server(written);
        }
        let ops: Vec<&str> = frames
            .iter()
            .map(|written| written["op"].as_str().unwrap())
            .collect();
        for op in ServerFrame::OPS {
            assert!(ops.contains(&op), "no example of {op}");
        }
    }

    #[test]
    fn a_frame_says_which_subscription_it_is_about() {
        let about = |written: Json| server(&written).subscription();
        let feed = feed();
        assert_eq!(
            about(json!({ "op": "resync", "id": 9, "feed": feed })),
            Some(9)
        );
        assert_eq!(
            about(json!({ "op": "error", "id": 4, "code": "bad_feed", "detail": "" })),
            Some(4)
        );
        assert_eq!(
            about(json!({ "op": "error", "code": "bad_feed", "detail": "" })),
            None
        );
        assert_eq!(about(json!({ "op": "pong", "n": 1 })), None);
    }

    #[test]
    fn the_server_may_say_more_than_this_client_knows_and_not_less() {
        let more = json!({ "op": "ready", "connection_id": "c7", "user_id": B, "server_time": AT,
                           "access_expires_at": AT, "heartbeat_secs": 25, "build": "b", "region": "eu" });
        assert!(ServerFrame::decode(&more.to_string()).is_ok());
        let less = json!({ "op": "ready", "connection_id": "c7", "user_id": B, "server_time": AT, "build": "b" });
        assert!(matches!(
            ServerFrame::decode(&less.to_string()),
            Err(FrameFault::Members { .. })
        ));
        // A feed nobody could have made is not read.
        let nonsense = json!({ "op": "resync", "id": 1, "feed": "weather" });
        assert!(matches!(
            ServerFrame::decode(&nonsense.to_string()),
            Err(FrameFault::Members { .. })
        ));
    }

    #[test]
    fn the_data_of_a_sync_or_an_event_frame_is_read_as_the_feed_defines_it() {
        let data = SyncData::Thread(ThreadData {
            items: Some(Vec::new()),
            ..ThreadData::default()
        });
        let thread_feed = Feed::Thread(id());
        let frame = SyncFrame::new(1, thread_feed.clone(), 412, 414, &data).unwrap();
        assert_eq!(
            frame.data,
            serde_json::from_value::<DataObject>(json!({ "items": [] })).unwrap()
        );
        assert_eq!(frame.data().unwrap(), data);

        let notice = EventData::User(UserEvent::Favorites);
        let event = EventFrame::new(2, Feed::User, &notice).unwrap();
        assert_eq!(
            Json::Object(event.data.clone()),
            json!({ "what": "favorites" })
        );
        assert_eq!(event.data().unwrap(), notice);

        // A feed that has no rows has no sync frames, and one without events has none.
        let misplaced = SyncFrame {
            id: 1,
            feed: Feed::EnvStats(id()),
            prev: 0,
            seq: 1,
            data: DataObject::new(),
        };
        assert!(matches!(misplaced.data(), Err(DataError::NotCarried(_))));
        let silent = EventFrame {
            id: 1,
            feed: thread_feed,
            data: DataObject::new(),
        };
        assert!(matches!(silent.data(), Err(DataError::NotCarried(_))));
    }

    #[test]
    fn a_process_event_reads_through_a_frame_end_to_end() {
        let written = json!({
            "op": "event", "id": 5, "feed": format!("env:{A}:procs"),
            "data": { "as_of": AT, "processes": [process()] }
        });
        let ServerFrame::Event(event) = server(&written) else {
            panic!("not an event")
        };
        let EventData::Procs(crate::feeds::Watched::Seen(seen)) = event.data().unwrap() else {
            panic!("not processes")
        };
        assert_eq!(seen.processes[0].pid, 4711);
    }

    #[test]
    fn every_operation_is_listed_once_and_each_frame_names_one_of_them() {
        let mut ops = ClientFrame::OPS.to_vec();
        ops.extend(ServerFrame::OPS);
        let mut sorted = ops.clone();
        sorted.sort_unstable();
        sorted.dedup();
        // `ping` and `pong` are told apart, and `ready`, `closed` and the rest belong to one side each.
        assert_eq!(sorted.len(), ops.len());
        assert_eq!((ClientFrame::OPS.len(), ServerFrame::OPS.len()), (7, 10));
    }
}
