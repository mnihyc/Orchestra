//! The frames of a harness's own protocol, kept for diagnosis: masked of anything that must not be stored, bounded, and read back newest first.
//!
//! An adapter reports the frames of its harness's protocol as `raw.frame` when the session was
//! opened asking for them. The engine keeps each original captured number once and retains its
//! segment cursor after the bytes expire, for as
//! long as `diagnostics.retention_days` says, so that a person who manages the thread can see
//! what went to the harness and what came back when an agent misbehaves.
//!
//! **Nothing that must not be stored is stored.** Before a frame is written every text in it, and
//! every name of a member, is masked as a chat message is: the patterns `redaction.patterns`
//! resolves to at the thread, and the values of the secrets of the thread's topic and of its
//! environment, unless `redaction.mask_secrets` is off there. What a secret is called does not
//! matter to a frame that carries one by a name that says so (an authorization header, a token, a
//! password, a key): the text that such a member holds is masked whatever it is. The same holds for a
//! pair that names a secret in one member and holds it in another, as lists of variables and headers
//! write them (`{ "name": "X_TOKEN", "value": "…" }`), and for `X_TOKEN=…` in a text, as a command
//! line or a list of variables writes it. A frame whose secrets
//! cannot be opened (the key that opens them is not installed, or they do not open) is dropped,
//! never written unmasked. A frame larger than [`FRAME_MAX_BYTES`] is kept as a note of its size
//! and its start, and a segment keeps at most [`SEGMENT_MAX_FRAMES`] of them.
//!
//! What is listed is masked once more with what is known when it is listed: a secret that was put
//! in after a frame was kept does not show in it. Where the secrets cannot be opened then, the
//! frames are listed as they were kept, which is masked already.
//!
//! What no rule above names is not masked: a secret that Orch does not hold, in a text that no
//! pattern matches, under a name that does not say it is one.

use crate::error::EngineError;
use chrono::{DateTime, Utc};
use orch_api_contract::threads::{RawDirection, RawEvent};
use orch_chat::redaction::{Pattern, Redactor};
use orch_db::Conn;
use orch_db::queries::raw_events::{self as rows, FROM_HARNESS, NewRawEvent, TO_HARNESS};
use orch_db::queries::threads::ThreadRow;
use orch_db::secrets::MasterKey;
use orch_link_contract::events::RawFrame;
use orch_settings::secrets::{self, Owner};
use orch_settings::{Place, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time};
use std::sync::OnceLock;

/// The most bytes of text of one frame that are kept whole: a frame can carry a file, and a
/// diagnostic is for reading what was asked and answered, not for keeping what was sent. A page of
/// the list holds up to [`PAGE_MAX`] frames, so this is also what keeps a reply to a size that a
/// client can take (500 frames of this size are 32 MiB at the very worst; the link carries no
/// event larger than 256 KiB, so a larger frame never arrives to be kept).
pub const FRAME_MAX_BYTES: usize = 64 * 1024;

/// How much of the start of a frame that was too large is kept, so that it can still be told what it was.
pub const FRAME_HEAD_BYTES: usize = 4 * 1024;

/// The most frames one segment keeps, newest first, so a harness that talks without end cannot
/// fill the table for the days of its retention and recent faults remain visible.
pub const SEGMENT_MAX_FRAMES: i64 = 20_000;

/// How long frames are kept where the setting says nothing: the shipped figure.
pub const DEFAULT_RETENTION_DAYS: i64 = 14;

/// How hard a frame is compressed: zstd's own default, which is quick and gets most of the gain.
const COMPRESSION_LEVEL: i32 = 3;

/// What a masked value is replaced by in a member that is named like a secret.
const MASKED: &str = "[masked]";

/// How many frames a page of a list holds when the request does not say.
pub const PAGE_DEFAULT: u32 = 100;
/// The most frames one page of a list holds.
pub const PAGE_MAX: u32 = 500;

/// What a member's name ends with when what it holds is a secret, in snake case (`accessToken` and
/// `Access-Token` are `access_token`): the name is one of these or ends with `_` and one of these,
/// so `refresh_token` and `OPENAI_API_KEY` are secrets and `input_tokens` and `session_key` are not.
const SECRET_NAME_ENDINGS: [&str; 17] = [
    "authorization",
    "cookie",
    "password",
    "passwd",
    "secret",
    "token",
    "credential",
    "credentials",
    "api_key",
    "apikey",
    "private_key",
    "secret_key",
    "access_key",
    "auth_key",
    "signing_key",
    "encryption_key",
    "master_key",
];

static MASTER_KEY: OnceLock<MasterKey> = OnceLock::new();

/// Gives the engine the key that opens the secrets of a place, so that the frames it keeps are masked
/// by them. Installed once, by the program that holds the key; false when one was.
pub fn install_master_key(key: MasterKey) -> bool {
    MASTER_KEY.set(key).is_ok()
}

fn master_key() -> Result<&'static MasterKey, String> {
    MASTER_KEY
        .get()
        .ok_or_else(|| "the key that opens secrets is not installed".to_owned())
}

/// What masks the frames of one thread.
#[derive(Debug)]
enum Masking {
    /// The patterns and the secrets are known.
    Ready(Redactor),
    /// They could not be opened: frames are dropped.
    Unavailable,
}

/// Keeps the frames of one segment of a thread, for the events of one delivery.
#[derive(Debug)]
pub struct Keeper {
    thread: Id,
    segment: Id,
    topic: Id,
    environment: Option<Id>,
    retention_days: Option<i64>,
    masking: Option<Masking>,
}

impl Keeper {
    /// A keeper for the frames of `segment` of the thread `row`. How long frames are kept and what
    /// masks them are looked up with the first frame.
    #[must_use]
    pub const fn new(row: &ThreadRow, segment: Id) -> Self {
        Self {
            thread: row.node_id,
            segment,
            topic: row.topic_id,
            environment: row.environment_id,
            retention_days: None,
            masking: None,
        }
    }

    async fn masking(&mut self, conn: &mut Conn) -> Result<&Masking, EngineError> {
        if self.masking.is_none() {
            self.masking = Some(masking_of(conn, self.thread, self.topic, self.environment).await?);
        }
        self.masking
            .as_ref()
            .ok_or_else(|| EngineError::unreadable("the masking of a thread was not made"))
    }

    /// Keeps `frame` once by its original number, reported at `at`, when it can be kept masked.
    pub async fn keep(
        &mut self,
        conn: &mut Conn,
        frame: &RawFrame,
        at: DateTime<Utc>,
    ) -> Result<(), EngineError> {
        let days = match self.retention_days {
            Some(days) => days,
            None => {
                let days = setting::<i64>(conn, Place::System, keys::DIAGNOSTICS_RETENTION_DAYS)
                    .await
                    .map_err(|error| EngineError::unreadable(error.to_string()))?
                    .unwrap_or(DEFAULT_RETENTION_DAYS);
                self.retention_days = Some(days);
                days
            }
        };
        if days <= 0 {
            return Ok(());
        }
        let Masking::Ready(redactor) = self.masking(conn).await? else {
            tracing::warn!(thread = %self.thread, "a frame of a harness was dropped: what masks it could not be made");
            return Ok(());
        };
        let (direction, payload) = packed(frame, redactor);
        rows::append(
            conn,
            &NewRawEvent {
                segment: self.segment,
                n: frame.n,
                at,
                direction,
                payload: &payload,
                at_most: SEGMENT_MAX_FRAMES,
            },
        )
        .await?;
        Ok(())
    }
}

/// What masks the frames of a thread: the patterns of its place, and the secrets of its topic and
/// of its environment unless the setting says they are not masked.
/// Masks a live native preview with the same current rules as its source traffic, dropping it if secrets cannot be opened.
pub(crate) async fn mask_live(
    conn: &mut Conn,
    row: &ThreadRow,
    environment: Option<Id>,
    mut value: Json,
) -> Result<Option<Json>, EngineError> {
    match masking_of(conn, row.node_id, row.topic_id, environment).await? {
        Masking::Ready(redactor) => {
            mask_value(&mut value, &redactor);
            Ok(Some(value))
        }
        Masking::Unavailable => Ok(None),
    }
}

/// What masks the text written from a thread's history, found once for a whole write: the same rules as [`mask_live`],
/// for a write that masks many texts and must not look the rules up for each. Nothing where the secrets cannot be opened.
pub(crate) async fn live_redactor(
    conn: &mut Conn,
    row: &ThreadRow,
    environment: Option<Id>,
) -> Result<Option<Redactor>, EngineError> {
    Ok(
        match masking_of(conn, row.node_id, row.topic_id, environment).await? {
            Masking::Ready(redactor) => Some(redactor),
            Masking::Unavailable => None,
        },
    )
}

/// A text masked by a redactor found with [`live_redactor`], as [`mask_live`] masks it.
pub(crate) fn mask_text(redactor: &Redactor, text: String) -> String {
    let mut value = Json::String(text);
    mask_value(&mut value, redactor);
    match value {
        Json::String(text) => text,
        other => other.to_string(),
    }
}

/// Masks summary material at a thread or topic before any producer sees it.
pub(crate) async fn mask_summary(
    conn: &mut Conn,
    node: Id,
    topic: Id,
    environment: Option<Id>,
    mut value: Json,
) -> Result<Option<Json>, EngineError> {
    match masking_of(conn, node, topic, environment).await? {
        Masking::Ready(redactor) => {
            mask_value(&mut value, &redactor);
            Ok(Some(value))
        }
        Masking::Unavailable => Ok(None),
    }
}

async fn masking_of(
    conn: &mut Conn,
    thread: Id,
    topic: Id,
    environment: Option<Id>,
) -> Result<Masking, EngineError> {
    let place = Place::Node(thread);
    let patterns: Vec<Pattern> = setting(conn, place, keys::REDACTION_PATTERNS)
        .await
        .map_err(|error| EngineError::unreadable(error.to_string()))?
        .unwrap_or_default();
    let mask_secrets: bool = setting(conn, place, keys::REDACTION_MASK_SECRETS)
        .await
        .map_err(|error| EngineError::unreadable(error.to_string()))?
        .unwrap_or(true);
    let mut values = Vec::new();
    if mask_secrets {
        let mut owners = vec![Owner::Node(topic)];
        owners.extend(environment.map(Owner::Environment));
        for owner in owners {
            let named = secrets::list(conn, owner)
                .await
                .map_err(|error| EngineError::unreadable(error.to_string()))?;
            if named.is_empty() {
                continue;
            }
            let Ok(key) = master_key() else {
                return Ok(Masking::Unavailable);
            };
            match secrets::open_resolved(conn, key, owner).await {
                Ok(opened) => values.extend(opened.into_iter().map(|each| each.value)),
                Err(_) => return Ok(Masking::Unavailable),
            }
        }
    }
    Ok(Masking::Ready(Redactor::new(&patterns, values)))
}

/// The word a direction is stored as: what an adapter calls the way a frame went, or `from_harness`
/// for a word that is neither (the connector's own notes, say), which then travels in the payload.
fn direction_of(word: &str) -> (&'static str, bool) {
    match word.trim().to_ascii_lowercase().as_str() {
        "to_harness" | "out" | "outgoing" | "sent" | "send" | "tx" | "to" | "request" => {
            (TO_HARNESS, false)
        }
        "from_harness" | "in" | "incoming" | "received" | "receive" | "rx" | "from"
        | "response" => (FROM_HARNESS, false),
        _ => (FROM_HARNESS, true),
    }
}

/// A name in snake case: a capital after a small letter or a digit begins a word, and a dash, a dot
/// or a space is an underscore.
fn snake_case(name: &str) -> String {
    let mut snake = String::with_capacity(name.len() + 4);
    let mut before: Option<char> = None;
    for each in name.chars() {
        if matches!(each, '-' | '.' | ' ') {
            snake.push('_');
        } else {
            let begins_word = each.is_ascii_uppercase()
                && before.is_some_and(|last| last.is_ascii_lowercase() || last.is_ascii_digit());
            if begins_word {
                snake.push('_');
            }
            snake.push(each.to_ascii_lowercase());
        }
        before = Some(each);
    }
    snake
}

fn is_secret_member(name: &str) -> bool {
    let snake = snake_case(name);
    SECRET_NAME_ENDINGS.iter().any(|ending| {
        snake == *ending
            || snake
                .strip_suffix(ending)
                .is_some_and(|rest| rest.ends_with('_'))
    })
}

/// The members that name what a pair holds, as a list of variables or headers writes it:
/// `{ "name": "X_TOKEN", "value": "…" }`.
const PAIR_NAMES: [&str; 5] = ["name", "key", "header", "variable", "field"];

/// The members that hold the value of such a pair.
const PAIR_VALUES: [&str; 4] = ["value", "val", "content", "data"];

/// Whether a name is one a shell takes for a variable: a letter or `_`, then letters, digits and `_`.
fn is_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|each| each.is_ascii_alphanumeric() || each == '_')
}

/// Masks the value of every `NAME=value` in a text whose name says it is a secret
/// (`X_TOKEN=…` in a command line, or in the list of variables a harness reports), and leaves the
/// rest of the text as it is.
fn mask_assignments(text: &str) -> String {
    if !text.contains('=') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    for piece in text.split_inclusive(char::is_whitespace) {
        let word = piece.trim_end_matches(char::is_whitespace);
        let space = &piece[word.len()..];
        match word.split_once('=') {
            Some((name, value))
                if !value.is_empty() && is_variable_name(name) && is_secret_member(name) =>
            {
                out.push_str(name);
                out.push('=');
                out.push_str(MASKED);
                out.push_str(space);
            }
            _ => out.push_str(piece),
        }
    }
    out
}

/// Whether an object is a pair that names a secret by one member and holds it in another.
fn is_secret_pair(members: &serde_json::Map<String, Json>) -> bool {
    members.iter().any(|(name, value)| {
        PAIR_NAMES.contains(&snake_case(name).as_str())
            && value.as_str().is_some_and(is_secret_member)
    })
}

/// Masks every text of a JSON value, every name of a member, the value of every member that is
/// named like a secret, the value of a pair whose name says it is one, and the value of every
/// `NAME=value` in a text whose name says so.
fn mask_value(value: &mut Json, redactor: &Redactor) {
    match value {
        Json::String(text) => *text = mask_assignments(&redactor.mask(text)),
        Json::Array(items) => {
            for each in items {
                mask_value(each, redactor);
            }
        }
        Json::Object(members) => {
            let display = members
                .get("name")
                .or_else(|| members.get("tool"))
                .and_then(Json::as_str)
                .is_some_and(|name| {
                    name == "use_display"
                        || name.ends_with("__use_display")
                        || name == "functions.use_display"
                });
            if display {
                // Vendors may encode tool arguments as JSON text; credentials stay masked in that form too.
                for member in ["arguments", "input"] {
                    if let Some(Json::String(text)) = members.get_mut(member) {
                        let mut arguments =
                            serde_json::from_str::<Json>(text).unwrap_or(Json::Null);
                        mask_value(&mut arguments, redactor);
                        *text = arguments.to_string();
                    }
                }
                if let Some(text) = members
                    .get_mut("input")
                    .and_then(|input| input.get_mut("inline"))
                    .and_then(|inline| inline.as_str())
                    .map(str::to_owned)
                {
                    let mut arguments = serde_json::from_str::<Json>(&text).unwrap_or(Json::Null);
                    mask_value(&mut arguments, redactor);
                    if let Some(input) = members.get_mut("input") {
                        input["inline"] = Json::String(arguments.to_string());
                    }
                }
            }
            let pair = is_secret_pair(members);
            let taken = std::mem::take(members);
            for (name, mut each) in taken {
                let holds_the_secret = is_secret_member(&name)
                    || (pair && PAIR_VALUES.contains(&snake_case(&name).as_str()));
                if holds_the_secret && each.is_string() {
                    each = Json::String(MASKED.to_owned());
                } else {
                    mask_value(&mut each, redactor);
                }
                members.insert(redactor.mask(&name), each);
            }
        }
        Json::Null | Json::Bool(_) | Json::Number(_) => {}
    }
}

/// The start of a text, cut at a character.
fn head_of(text: &str, bytes: usize) -> &str {
    let mut end = bytes.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// A frame as it is stored: its direction and the compressed text of its masked JSON.
fn packed(frame: &RawFrame, redactor: &Redactor) -> (&'static str, Vec<u8>) {
    let (direction, foreign) = direction_of(&frame.direction);
    let mut payload = frame.payload.clone();
    mask_value(&mut payload, redactor);
    if foreign {
        payload = serde_json::json!({
            "direction": redactor.mask(&frame.direction),
            "frame": payload,
        });
    }
    let mut text = payload.to_string();
    if text.len() > FRAME_MAX_BYTES {
        text = serde_json::json!({
            "truncated": true,
            "bytes": text.len(),
            "head": head_of(&text, FRAME_HEAD_BYTES),
        })
        .to_string();
    }
    // Compressing text in memory does not fail; a frame that would not compress is kept as it is.
    let compressed = zstd::encode_all(text.as_bytes(), COMPRESSION_LEVEL)
        .unwrap_or_else(|_| text.clone().into_bytes());
    (direction, compressed)
}

/// The JSON a stored frame holds.
fn unpacked(bytes: &[u8]) -> Result<Json, EngineError> {
    let text = zstd::decode_all(bytes).map_err(|error| {
        EngineError::unreadable(format!("a raw event does not unpack: {error}"))
    })?;
    serde_json::from_slice(&text)
        .map_err(|error| EngineError::unreadable(format!("a raw event does not read: {error}")))
}

/// A page of the frames of a segment, newest first.
#[derive(Debug, Clone, PartialEq)]
pub struct RawPage {
    /// The frames.
    pub events: Vec<RawEvent>,
    /// Whether older ones exist.
    pub has_older: bool,
}

/// Reads frames of a segment of `thread` back: the newest `limit` of those numbered below `before`,
/// newest first, masked once more with the patterns and the secrets of the thread's place as they
/// are now.
pub async fn page(
    conn: &mut Conn,
    thread: &ThreadRow,
    segment: Id,
    before: Option<i64>,
    limit: u32,
) -> Result<RawPage, EngineError> {
    let found = rows::newest(conn, segment, before, i64::from(limit)).await?;
    let has_older = match found.last() {
        Some(oldest) => rows::older_than(conn, segment, oldest.n).await?,
        None => false,
    };
    let masking = if found.is_empty() {
        None
    } else {
        Some(masking_of(conn, thread.node_id, thread.topic_id, thread.environment_id).await?)
    };
    let mut events = Vec::with_capacity(found.len());
    for row in found {
        let mut payload = unpacked(&row.payload)?;
        if let Some(Masking::Ready(redactor)) = &masking {
            mask_value(&mut payload, redactor);
        }
        events.push(RawEvent {
            n: row.n,
            at: Time::new(row.at),
            direction: if row.direction == TO_HARNESS {
                RawDirection::ToHarness
            } else {
                RawDirection::FromHarness
            },
            payload,
        });
    }
    Ok(RawPage { events, has_older })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn redactor() -> Redactor {
        Redactor::new(
            &[Pattern {
                name: "key".to_owned(),
                regex: r"sk-[a-z0-9]{8,}".to_owned(),
            }],
            vec!["hunter2-the-secret".to_owned()],
        )
    }

    #[test]
    fn a_secret_is_masked_wherever_a_frame_carries_it_in_a_text_or_in_the_name_of_a_member() {
        let mut frame = json!({
            "method": "thread/start",
            "params": { "env": { "hunter2-the-secret": "x" }, "prompt": ["use sk-abcdef123456 or hunter2-the-secret"] },
            "count": 3
        });
        mask_value(&mut frame, &redactor());
        let text = frame.to_string();
        assert!(
            !text.contains("hunter2") && !text.contains("sk-abcdef"),
            "{text}"
        );
        assert!(text.contains("[masked]"));
        assert_eq!(frame["count"], 3, "what is no text is left alone");
    }

    #[test]
    fn a_display_password_is_masked_in_vendor_json_text_and_raw_argument_previews() {
        for mut frame in [
            json!({"name":"use_display","arguments":json!({"endpoint":"localhost:5900","password":"private-display-password"}).to_string()}),
            json!({"name":"mcp__orch__use_display","input":{"inline":json!({"endpoint":"localhost:5900","password":"private-display-password"}).to_string()}}),
        ] {
            mask_value(&mut frame, &Redactor::none());
            assert!(!frame.to_string().contains("private-display-password"));
            assert!(frame.to_string().contains("localhost:5900"));
        }
    }

    #[test]
    fn a_member_named_like_a_secret_is_masked_whatever_it_holds() {
        let mut frame = json!({
            "headers": { "Authorization": "Bearer abc.def.ghi", "X-Api-Key": "k-123", "Content-Type": "json", "Set-Cookie": "sid=1" },
            "login": { "access_token": "tok", "refresh-token": "tok2", "accessToken": "tok3", "expires_in": 3600, "input_tokens": 12, "token_type": "Bearer" },
            "env": { "OPENAI_API_KEY": "sk-1", "AWS_SECRET_ACCESS_KEY": "aws", "session_key": "orch-session", "key": "Enter" },
            "password": "pw"
        });
        mask_value(&mut frame, &Redactor::none());
        assert_eq!(frame["headers"]["Authorization"], "[masked]");
        assert_eq!(frame["headers"]["X-Api-Key"], "[masked]");
        assert_eq!(frame["headers"]["Set-Cookie"], "[masked]");
        assert_eq!(frame["headers"]["Content-Type"], "json");
        assert_eq!(frame["login"]["access_token"], "[masked]");
        assert_eq!(frame["login"]["refresh-token"], "[masked]");
        assert_eq!(frame["login"]["accessToken"], "[masked]");
        assert_eq!(frame["login"]["expires_in"], 3600);
        assert_eq!(
            frame["login"]["input_tokens"], 12,
            "a count of tokens is no token"
        );
        assert_eq!(
            frame["login"]["token_type"], "Bearer",
            "the kind of a token is no token"
        );
        assert_eq!(frame["env"]["OPENAI_API_KEY"], "[masked]");
        assert_eq!(frame["env"]["AWS_SECRET_ACCESS_KEY"], "[masked]");
        assert_eq!(
            frame["env"]["session_key"], "orch-session",
            "an identifier is no secret"
        );
        assert_eq!(
            frame["env"]["key"], "Enter",
            "a key of a keyboard is no secret"
        );
        assert_eq!(frame["password"], "[masked]");
    }

    #[test]
    fn a_secret_named_by_one_member_and_held_in_another_is_masked_at_any_depth() {
        let mut frame = json!({
            "env": [
                { "name": "X_TOKEN", "value": "unique-secret-1" },
                { "name": "PATH", "value": "/usr/bin" },
                { "Name": "GITHUB-TOKEN", "Value": "unique-secret-2" },
            ],
            "headers": [{ "header": "X-Api-Key", "content": "unique-secret-3" }],
            "nested": { "deeper": [{ "variable": "DB_PASSWORD", "val": "unique-secret-4" }] },
            "args": ["X_TOKEN=unique-secret-5 cargo test", "COLOR=always"],
            "cmd": "env AWS_SECRET_ACCESS_KEY=unique-secret-6  ls\n",
        });
        mask_value(&mut frame, &Redactor::none());
        let text = frame.to_string();
        assert!(!text.contains("unique-secret"), "{text}");
        assert_eq!(
            frame["env"][0]["name"], "X_TOKEN",
            "the name of a pair stays readable"
        );
        assert_eq!(
            frame["env"][1]["value"], "/usr/bin",
            "a pair that names no secret stays"
        );
        assert_eq!(frame["args"][0], "X_TOKEN=[masked] cargo test");
        assert_eq!(frame["args"][1], "COLOR=always");
        assert_eq!(frame["cmd"], "env AWS_SECRET_ACCESS_KEY=[masked]  ls\n");
    }

    #[test]
    fn a_direction_is_one_of_two_and_a_word_that_is_neither_travels_in_the_payload() {
        assert_eq!(direction_of("out"), (TO_HARNESS, false));
        assert_eq!(direction_of("In"), (FROM_HARNESS, false));
        assert_eq!(direction_of("to_harness"), (TO_HARNESS, false));
        assert_eq!(direction_of("connector"), (FROM_HARNESS, true));
        let (direction, bytes) = packed(
            &RawFrame {
                n: 1,
                direction: "connector".to_owned(),
                payload: json!({ "policy": "x" }),
            },
            &Redactor::none(),
        );
        assert_eq!(direction, FROM_HARNESS);
        assert_eq!(
            unpacked(&bytes).unwrap(),
            json!({ "direction": "connector", "frame": { "policy": "x" } })
        );
    }

    #[test]
    fn a_frame_larger_than_may_be_kept_is_kept_as_a_note_of_its_size_and_start() {
        let big = "é".repeat(FRAME_MAX_BYTES);
        let (_, bytes) = packed(
            &RawFrame {
                n: 1,
                direction: "out".to_owned(),
                payload: json!({ "file": big }),
            },
            &Redactor::none(),
        );
        let kept = unpacked(&bytes).unwrap();
        assert_eq!(kept["truncated"], true);
        assert!(kept["bytes"].as_u64().unwrap() > FRAME_MAX_BYTES as u64);
        let head = kept["head"].as_str().unwrap();
        assert!(head.len() <= FRAME_HEAD_BYTES && head.starts_with("{\"file\":\"é"));
    }

    #[test]
    fn what_is_packed_is_unpacked_to_the_same_json() {
        let payload = json!({ "a": [1, 2, { "b": null }], "c": "text" });
        let (_, bytes) = packed(
            &RawFrame {
                n: 1,
                direction: "in".to_owned(),
                payload: payload.clone(),
            },
            &Redactor::none(),
        );
        assert_eq!(unpacked(&bytes).unwrap(), payload);
    }
}
